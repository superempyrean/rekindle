//! Source maps for JavaScript and TypeScript plugins.
//!
//! `rekindle build` saves the bundler's map next to the component
//! (`<id>.js.map`), with `sources` relative to the project. The runtime's log
//! buffer uses it to rewrite stack frames in a plugin's log lines, so an error
//! points at `src/index.ts:13:15` instead of the bundle (`/entry.mjs:601:26`).

use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

/// A decoded source map: for each generated line, its segments sorted by
/// column.
#[derive(Debug, Default)]
pub struct SourceMap {
    sources: Vec<String>,
    lines: Vec<Vec<Segment>>,
}

#[derive(Debug, Clone, Copy)]
struct Segment {
    col: u32,
    source: u32,
    line: u32,
    src_col: u32,
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    sources: Vec<Option<String>>,
    mappings: String,
}

impl SourceMap {
    pub fn read(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text)
    }

    fn parse(json: &str) -> Result<Self> {
        let raw: Raw = serde_json::from_str(json).context("parsing a source map")?;
        let sources = raw
            .sources
            .into_iter()
            .map(Option::unwrap_or_default)
            .collect();
        let mut lines = Vec::new();
        let (mut source, mut line, mut src_col) = (0i64, 0i64, 0i64);
        for text in raw.mappings.split(';') {
            let mut col = 0i64;
            let mut segs = Vec::new();
            for seg in text.split(',').filter(|s| !s.is_empty()) {
                let f = vlq(seg).with_context(|| format!("bad mapping segment `{seg}`"))?;
                col += f[0];
                if f.len() >= 4 {
                    source += f[1];
                    line += f[2];
                    src_col += f[3];
                    let n = |v: i64| u32::try_from(v).unwrap_or(0);
                    segs.push(Segment {
                        col: n(col),
                        source: n(source),
                        line: n(line),
                        src_col: n(src_col),
                    });
                }
            }
            segs.sort_by_key(|s| s.col);
            lines.push(segs);
        }
        Ok(Self { sources, lines })
    }

    /// The original position of a generated one (all 1-based), if mapped.
    fn lookup(&self, line: u32, col: u32) -> Option<(&str, u32, u32)> {
        let segs = self.lines.get(line.checked_sub(1)? as usize)?;
        let col = col.saturating_sub(1);
        let i = segs.partition_point(|s| s.col <= col).checked_sub(1)?;
        let s = segs[i];
        Some((
            self.sources.get(s.source as usize)?.as_str(),
            s.line + 1,
            s.src_col + 1,
        ))
    }

    /// Rewrite every `entry.mjs:<line>:<col>` frame in `text`, the name the
    /// engine gives the bundled module, to the original file and position.
    /// Lines whose frame is rekindle's own wrapper code (mapped to
    /// [`INTERNAL`]) are dropped.
    pub fn rewrite(&self, text: &str) -> String {
        let lines: Vec<String> = text
            .split('\n')
            .filter_map(|l| self.rewrite_line(l))
            .collect();
        lines.join("\n")
    }

    fn rewrite_line(&self, text: &str) -> Option<String> {
        const NAME: &str = "entry.mjs:";
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(i) = rest.find(NAME) {
            let after = &rest[i + NAME.len()..];
            let (line, tail) = digits(after);
            let mapped = tail.strip_prefix(':').and_then(|t| {
                let (col, tail) = digits(t);
                let (file, l, c) = self.lookup(line?, col?)?;
                Some((file, format!("{file}:{l}:{c}"), tail))
            });
            if mapped.as_ref().is_some_and(|(file, ..)| *file == INTERNAL) {
                return None;
            }
            let mapped = mapped.map(|(_, pos, tail)| (pos, tail));
            // Keep what came before the module name, minus a leading `/`.
            let before = &rest[..i];
            if let Some((pos, tail)) = mapped {
                out.push_str(before.strip_suffix('/').unwrap_or(before));
                out.push_str(&pos);
                rest = tail;
            } else {
                out.push_str(&rest[..i + NAME.len()]);
                rest = after;
            }
        }
        out.push_str(rest);
        Some(out)
    }
}

/// The source name for rekindle's own generated code (the entry module and
/// the sandbox prelude) in a saved map.
pub const INTERNAL: &str = "(rekindle)";

fn digits(s: &str) -> (Option<u32>, &str) {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    (s[..n].parse().ok(), &s[n..])
}

/// One segment's base64 VLQ fields.
fn vlq(seg: &str) -> Option<Vec<i64>> {
    let mut out = Vec::new();
    let (mut value, mut shift) = (0i64, 0u32);
    for b in seg.bytes() {
        let d = i64::from(match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        });
        value += (d & 31) << shift;
        if d & 32 != 0 {
            shift += 5;
            if shift > 60 {
                return None;
            }
        } else {
            let v = value >> 1;
            out.push(if value & 1 == 1 { -v } else { v });
            value = 0;
            shift = 0;
        }
    }
    (shift == 0 && !out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // From rolldown, for a two-module bundle (the map `bundle.config.mjs`
    // saves): `throw` on line 6 of the bundle is line 7 of src/index.ts.
    const MAP: &str = r#"{"version":3,"sources":["src/index.ts","entry.mjs"],"mappings":";AACA,SAAgB,WAAiB;CAE/B,OADiB,KAAK,MAAM,IACrB,CAAC;AACV;AACA,SAAS,OAAO,GAAgB;CAC9B,MAAM,IAAI,MAAM,QAAQ,EAAE,GAAG;AAC/B;;ACNA,MAAa,YAAY,EAAE,SAAS"}"#;

    #[test]
    fn decodes_vlq() {
        assert_eq!(vlq("AAAA"), Some(vec![0, 0, 0, 0]));
        assert_eq!(vlq("SAAgB"), Some(vec![9, 0, 0, 16]));
        assert_eq!(vlq("D"), Some(vec![-1]));
        assert_eq!(vlq("g"), None, "a continuation bit with nothing after it");
    }

    #[test]
    fn maps_a_frame_back_to_the_source() {
        let m = SourceMap::parse(MAP).unwrap();
        assert_eq!(m.lookup(6, 2), Some(("src/index.ts", 7, 3)));
        let trace = "Error: boom\n    at helper (/entry.mjs:6:8)\n    at <input>:1:1";
        assert_eq!(
            m.rewrite(trace),
            "Error: boom\n    at helper (src/index.ts:7:9)\n    at <input>:1:1"
        );
        // Unmapped positions are left alone.
        assert_eq!(
            m.rewrite("at x (/entry.mjs:99:1)"),
            "at x (/entry.mjs:99:1)"
        );
    }

    #[test]
    fn drops_rekindles_own_frames() {
        let map = MAP.replace(r#""entry.mjs"]"#, r#""(rekindle)"]"#);
        let m = SourceMap::parse(&map).unwrap();
        let trace = "Error: boom\n    at helper (/entry.mjs:6:8)\n    at call (/entry.mjs:9:7)";
        assert_eq!(
            m.rewrite(trace),
            "Error: boom\n    at helper (src/index.ts:7:9)"
        );
    }
}
