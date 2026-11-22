//! Dropping the hooks a Rust plugin can't be called through.
//!
//! wit-bindgen exports every hook in the world, and each export carries the
//! glue that reads its arguments out of the plugin's memory. For the hooks
//! that take an `event` or a message, that glue is the biggest code in a
//! small plugin (around 28 KB of wasm each) and most of its compile time.
//! The host never calls a hook whose capability the manifest doesn't
//! request (`runtime::hook_capability`), so `rekindle build` removes those
//! exports from the world and the module, and stubs out every function only
//! they reached. Functions keep their indices, so nothing else in the module
//! changes.

use std::collections::HashSet;
use std::ops::Range;

use anyhow::{Context, Result};
use wasmparser::{ElementItems, ExternalKind, Operator, OperatorsReader, Parser, Payload, TypeRef};

use crate::manifest::Manifest;
use crate::runtime::{HOOKS, hook_capability};

/// `module` without the hooks `manifest` rules out. `module` is a
/// wit-bindgen core module with its `component-type` section.
pub(super) fn unused_hooks(module: &[u8], manifest: &Manifest) -> Result<Vec<u8>> {
    let ruled_out: Vec<&str> = HOOKS
        .iter()
        .copied()
        .filter(|h| hook_capability(h).is_some_and(|c| !manifest.requests(c)))
        .collect();
    let (stripped, mut bindgen) =
        wit_component::metadata::decode(module).context("reading the module's component type")?;
    let Some(stripped) = stripped else {
        return Ok(module.to_vec());
    };
    let resolve = &mut bindgen.resolve;
    let world = &mut resolve.worlds[bindgen.world];
    let mut dropped: Vec<&str> = Vec::new();
    world.exports.retain(|key, _| {
        let wit_parser::WorldKey::Interface(id) = key else {
            return true;
        };
        let iface = &resolve.interfaces[*id];
        let ours = iface
            .package
            .is_some_and(|p| resolve.packages[p].name.namespace == "rekindle");
        let hook = iface
            .name
            .as_deref()
            .and_then(|name| ruled_out.iter().find(|h| **h == name));
        match hook {
            Some(h) if ours => {
                dropped.push(h);
                false
            }
            _ => true,
        }
    });
    if dropped.is_empty() {
        return Ok(module.to_vec());
    }
    let exported_by = |name: &str| {
        let name = name.strip_prefix("cabi_post_").unwrap_or(name);
        dropped
            .iter()
            .any(|h| name.starts_with(&format!("rekindle:plugin/{h}@")))
    };
    let mut out = strip(&stripped, exported_by)?;
    wit_component::embed_component_metadata(
        &mut out,
        &bindgen.resolve,
        bindgen.world,
        wit_component::StringEncoding::UTF8,
        false,
    )?;
    Ok(out)
}

/// `module` without the exports `drop` names, and with every function that
/// nothing kept reaches stubbed to `unreachable`.
fn strip(module: &[u8], drop: impl Fn(&str) -> bool) -> Result<Vec<u8>> {
    let code = CodeMap::read(module, &drop)?;
    let live = code.reachable();
    let mut out = wasm_encoder::Module::new();
    for payload in Parser::new(0).parse_all(module) {
        let payload = payload?;
        match &payload {
            Payload::ExportSection(s) => {
                let mut exports = wasm_encoder::ExportSection::new();
                for e in s.clone() {
                    let e = e?;
                    if is_func(e.kind) && drop(e.name) {
                        continue;
                    }
                    let kind = match e.kind {
                        ExternalKind::Func | ExternalKind::FuncExact => {
                            wasm_encoder::ExportKind::Func
                        }
                        ExternalKind::Table => wasm_encoder::ExportKind::Table,
                        ExternalKind::Memory => wasm_encoder::ExportKind::Memory,
                        ExternalKind::Global => wasm_encoder::ExportKind::Global,
                        ExternalKind::Tag => wasm_encoder::ExportKind::Tag,
                    };
                    exports.export(e.name, kind, e.index);
                }
                out.section(&exports);
            }
            Payload::CodeSectionStart { .. } => {
                let mut section = wasm_encoder::CodeSection::new();
                for (i, (range, _)) in code.bodies.iter().enumerate() {
                    if live.contains(&(code.imported + i as u32)) {
                        section.raw(&module[range.clone()]);
                    } else {
                        let mut f = wasm_encoder::Function::new([]);
                        f.instruction(&wasm_encoder::Instruction::Unreachable);
                        f.instruction(&wasm_encoder::Instruction::End);
                        section.function(&f);
                    }
                }
                out.section(&section);
            }
            Payload::CodeSectionEntry(_) => {}
            other => {
                if let Some((id, range)) = other.as_section() {
                    out.section(&wasm_encoder::RawSection {
                        id,
                        data: &module[range.start as usize..range.end as usize],
                    });
                }
            }
        }
    }
    Ok(out.finish())
}

/// The call graph of a module's own functions, and where it starts.
struct CodeMap {
    /// Imported functions, which come first in the function index space.
    imported: u32,
    /// The kept exports, the start function, and the functions tables and
    /// globals refer to.
    roots: Vec<u32>,
    /// Each defined function's body, and the functions it calls or refers to.
    bodies: Vec<(Range<usize>, Vec<u32>)>,
}

impl CodeMap {
    fn read(module: &[u8], drop: impl Fn(&str) -> bool) -> Result<Self> {
        let mut map = CodeMap {
            imported: 0,
            roots: Vec::new(),
            bodies: Vec::new(),
        };
        for payload in Parser::new(0).parse_all(module) {
            match payload? {
                Payload::ImportSection(s) => {
                    for imp in s.into_imports() {
                        if matches!(imp?.ty, TypeRef::Func(_) | TypeRef::FuncExact(_)) {
                            map.imported += 1;
                        }
                    }
                }
                Payload::ExportSection(s) => {
                    for e in s {
                        let e = e?;
                        if is_func(e.kind) && !drop(e.name) {
                            map.roots.push(e.index);
                        }
                    }
                }
                Payload::StartSection { func, .. } => map.roots.push(func),
                Payload::ElementSection(s) => {
                    for el in s {
                        match el?.items {
                            ElementItems::Functions(fs) => {
                                for f in fs {
                                    map.roots.push(f?);
                                }
                            }
                            ElementItems::Expressions(_, es) => {
                                for e in es {
                                    ref_funcs(e?.get_operators_reader(), &mut map.roots)?;
                                }
                            }
                        }
                    }
                }
                Payload::GlobalSection(s) => {
                    for g in s {
                        ref_funcs(g?.init_expr.get_operators_reader(), &mut map.roots)?;
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    let mut calls = Vec::new();
                    for op in body.get_operators_reader()? {
                        match op? {
                            Operator::Call { function_index }
                            | Operator::ReturnCall { function_index }
                            | Operator::RefFunc { function_index } => calls.push(function_index),
                            _ => {}
                        }
                    }
                    let r = body.range();
                    map.bodies.push((r.start as usize..r.end as usize, calls));
                }
                _ => {}
            }
        }
        Ok(map)
    }

    /// Every function the roots reach.
    fn reachable(&self) -> HashSet<u32> {
        let mut live = HashSet::new();
        let mut stack = self.roots.clone();
        while let Some(f) = stack.pop() {
            if !live.insert(f) {
                continue;
            }
            let body = f
                .checked_sub(self.imported)
                .and_then(|i| self.bodies.get(i as usize));
            if let Some((_, calls)) = body {
                stack.extend(calls.iter().copied().filter(|c| !live.contains(c)));
            }
        }
        live
    }
}

fn is_func(kind: ExternalKind) -> bool {
    matches!(kind, ExternalKind::Func | ExternalKind::FuncExact)
}

/// The functions a constant expression refers to (`ref.func`).
fn ref_funcs(ops: OperatorsReader, out: &mut Vec<u32>) -> Result<()> {
    for op in ops {
        if let Operator::RefFunc { function_index } = op? {
            out.push(function_index);
        }
    }
    Ok(())
}
