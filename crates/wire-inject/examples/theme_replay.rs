//! Replay a client's recorded turns through the engine with a theme, and
//! report what its recolours reached in each palette the guest named.
//!
//! ```text
//! theme_replay <wire.capture> <logcat> <palette.json> <file.theme.json> [variant]
//! ```
//!
//! `wire.capture` holds the turns (`rekindle_wire::capture`). The logcat
//! holds the host's `theme: palette #n dark=.. turn=T key=aarrggbb ..`
//! lines,
//! each the palette the guest named before turn `T`. `palette.json` is the
//! build's theme binding. Each palette is reported twice: with the
//! recolours' roles, and with every recolour reaching every colour cell,
//! as a recolour by value alone did.

use std::collections::BTreeMap;

use rekindle_wire::{PropId, Value};
use rekindle_wire_inject::theme::{Palette, ThemeBinding, reach};
use rekindle_wire_inject::{ColorRole, Engine, Recolor, Work};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!(
            "usage: theme_replay <wire.capture> <logcat> <palette.json> <file.theme.json> [variant]"
        );
        std::process::exit(2);
    }
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{p}: {e}"));
    let capture = std::fs::read(&args[1]).expect("capture");
    let (records, _) = rekindle_wire::capture::parse(&capture).expect("a wire capture");
    let binding = ThemeBinding::from_json(&read(&args[3])).expect("a theme binding");
    let overrides = theme_tokens(&read(&args[4]), args.get(5).map(String::as_str));
    let palettes = palettes(&read(&args[2]), binding.dark);
    println!(
        "{} turns, {} palettes named, {} tokens set by the theme",
        records.len(),
        palettes.len(),
        overrides.len()
    );

    let work = |palette: Option<&Palette>, roles: bool| {
        let recolors = binding
            .recolors(palette, &overrides)
            .into_iter()
            .map(|r| if roles { r } else { Recolor::new(r.from, r.to) })
            .collect();
        Work {
            recolors,
            ..Work::default()
        }
    };
    let mut with_roles = Engine::new(work(None, true)).unwrap();
    let mut by_value = Engine::new(work(None, false)).unwrap();
    let mut shown: Option<&(u64, Palette)> = None;
    let mut next = palettes.iter().peekable();
    let report = |shown: Option<&(u64, Palette)>, a: &Engine, b: &Engine, upto: u64| {
        let palette = shown.map(|(_, p)| p);
        let values = binding.values(palette);
        let roles = binding.recolors(palette, &overrides);
        let theme = match palette {
            Some(p) => binding.theme_of(p).unwrap_or(if p.is_dark() {
                "an unnamed dark"
            } else {
                "an unnamed light"
            }),
            None => "no",
        };
        println!();
        println!("== turns up to {upto}: {theme} theme ==");
        for v in &values {
            let to = overrides.get(&v.token);
            println!(
                "  {:<28} {:<7?} #{:06x} ({}){}",
                v.token,
                v.role,
                v.value,
                if v.live { "live" } else { "table" },
                to.map(|c| format!(" -> #{:06x}", c >> 8))
                    .unwrap_or_default()
            );
        }
        println!("  with roles: {}", reach(a, &roles, &values));
        println!("  by value:   {}", reach(b, &roles, &values));
    };
    // The guest's colour cells at the end of each palette's turns.
    let mut ends = Vec::new();
    for r in &records {
        while let Some(p) = next.next_if(|(t, _)| *t <= r.turn) {
            if shown.is_some() || r.turn > 0 {
                report(shown, &with_roles, &by_value, r.turn);
                ends.push(colours(with_roles.guest()));
            }
            shown = Some(p);
            with_roles.set_work(work(Some(&p.1), true)).unwrap();
            by_value.set_work(work(Some(&p.1), false)).unwrap();
        }
        for e in [&mut with_roles, &mut by_value] {
            if let Err(err) = e.filter(&r.bytes) {
                eprintln!("turn {}: {err}", r.turn);
            }
        }
    }
    let last = records.last().map_or(0, |r| r.turn);
    report(shown, &with_roles, &by_value, last);
    ends.push(colours(with_roles.guest()));
    switches(&ends, with_roles.guest());
}

/// What each cell held before and after each switch of palette, from the
/// guest's colour cells at the end of each palette's turns: a token's
/// colour in one palette beside its colour in the other.
fn switches(ends: &[BTreeMap<(rekindle_wire::NodeId, PropId), u32>], tree: &rekindle_wire::Tree) {
    for (i, w) in ends.windows(2).enumerate() {
        let mut pairs: BTreeMap<(ColorRole, u32, u32), Vec<_>> = BTreeMap::new();
        for (cell, before) in &w[0] {
            if let Some(after) = w[1].get(cell)
                && after != before
            {
                let role = ColorRole::of(cell.1).unwrap_or_default();
                pairs
                    .entry((role, *before, *after))
                    .or_default()
                    .push(*cell);
            }
        }
        let mut pairs: Vec<_> = pairs.into_iter().collect();
        pairs.sort_by_key(|(_, cells)| std::cmp::Reverse(cells.len()));
        println!();
        println!("== cells that changed colour at switch {} ==", i + 1);
        for ((role, a, b), cells) in pairs.iter().take(40) {
            let seen: Vec<String> = cells
                .iter()
                .take(4)
                .map(|(n, p)| describe(tree, *n, *p))
                .collect();
            println!(
                "  {role:<7?} #{a:06x} -> #{b:06x}  {:>3}  {}",
                cells.len(),
                seen.join(" | ")
            );
        }
    }
}

/// A node for a person to find: its cell, kind, and the first text in it
/// or under it.
fn describe(tree: &rekindle_wire::Tree, node: rekindle_wire::NodeId, prop: PropId) -> String {
    fn text(tree: &rekindle_wire::Tree, node: rekindle_wire::NodeId, depth: u8) -> Option<String> {
        for p in [PropId(499), PropId(11), PropId(15)] {
            if let Some(Value::Str(s)) = tree.props().get(node, p) {
                return Some(s.chars().take(24).collect());
            }
        }
        if depth == 0 {
            return None;
        }
        let n = tree.node(node)?;
        n.children.iter().find_map(|c| text(tree, *c, depth - 1))
    }
    let kind = tree
        .node(node)
        .map_or_else(|| "?".to_string(), |n| format!("{:?}", n.kind));
    format!(
        "{}.{} {kind} {:?}",
        node,
        prop.name().unwrap_or("?"),
        text(tree, node, 3).unwrap_or_default()
    )
}

/// Every opaque colour cell of the tree's nodes, `0xRRGGBB`.
fn colours(tree: &rekindle_wire::Tree) -> BTreeMap<(rekindle_wire::NodeId, PropId), u32> {
    let mut out = BTreeMap::new();
    for n in tree.nodes() {
        for p in rekindle_wire_inject::COLOR_PROPS {
            if let Some(c) = tree.props().get(n.id, *p).and_then(Value::as_color)
                && c >> 24 == 0xff
            {
                out.insert((n.id, *p), c & 0x00ff_ffff);
            }
        }
    }
    out
}

/// The palettes in the host's log lines, by the turn they came before.
fn palettes(log: &str, dark: PropId) -> Vec<(u64, Palette)> {
    let mut out = Vec::new();
    for line in log.lines() {
        let Some(rest) = line.split("theme: palette #").nth(1) else {
            continue;
        };
        let mut fields = Vec::new();
        let mut turn = 0;
        for w in rest.split_whitespace().skip(1) {
            let Some((k, v)) = w.split_once('=') else {
                continue;
            };
            match k {
                "dark" => fields.push((dark, Value::Bool(v.contains("true")))),
                "turn" => turn = v.parse().unwrap_or(0),
                _ => {
                    // `aarrggbb`, or `rrggbb` for an opaque colour.
                    if let (Ok(k), Ok(c)) = (k.parse::<u16>(), u32::from_str_radix(v, 16)) {
                        let c = if v.len() == 8 { c } else { 0xff00_0000 | c };
                        fields.push((PropId(k), Value::color(c)));
                    }
                }
            }
        }
        if let Some(p) = Palette::from_record(&Value::Object(fields), dark) {
            out.push((turn, p));
        }
    }
    out
}

/// A `.theme.json`'s tokens, with `variant`'s (or its default variant's)
/// over them, as `0xRRGGBBAA`.
fn theme_tokens(json: &str, variant: Option<&str>) -> BTreeMap<String, u32> {
    let v: serde_json::Value = serde_json::from_str(json).expect("a theme file");
    let mut out = BTreeMap::new();
    let mut add = |tokens: Option<&serde_json::Value>| {
        for (k, c) in tokens.and_then(|t| t.as_object()).into_iter().flatten() {
            let h = c.as_str().unwrap_or("").trim_start_matches('#');
            let c = match h.len() {
                6 => u32::from_str_radix(h, 16).ok().map(|c| (c << 8) | 0xff),
                8 => u32::from_str_radix(h, 16).ok(),
                _ => None,
            };
            if let Some(c) = c {
                out.insert(k.clone(), c);
            }
        }
    };
    add(v.get("tokens"));
    let want = variant.or_else(|| v.get("default-variant").and_then(|d| d.as_str()));
    let variants = v.get("variants").and_then(|v| v.as_array());
    let chosen = variants.and_then(|vs| {
        vs.iter()
            .find(|x| want.is_none_or(|w| x.get("name").and_then(|n| n.as_str()) == Some(w)))
    });
    add(chosen.and_then(|c| c.get("tokens")));
    out
}
