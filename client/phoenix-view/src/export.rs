//! `--export-assets`: the drawables a set of captures names and the APK's
//! fonts, written out for the theme playground (`playground/` in the root
//! workspace), which can't read an APK.
//!
//! `<dir>/drawables.json` maps each drawable name a capture's cells give as
//! `res://<name>` to `{"uri", "width", "height"}`: a `data:` URI of the
//! drawable as phoenix-view draws it, untinted, and its size in dp when it
//! has one. The fonts go to `<dir>/fonts/` under their names in the APK,
//! listed in `<dir>/fonts.json` with the family and weight a page declares
//! them under. Everything comes from the user's own APK, and the playground
//! keeps it out of git.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context as _, Result};
use base64::Engine as _;
use rekindle_wire::{NodeOp, Turn, Value};
use serde_json::{Map, Value as Json, json};

use crate::apk::Apk;
use crate::drawable;
use crate::res::Want;

/// Every `res://` name in the cells the captures' turns write.
fn names(captures: &[&Path]) -> Result<BTreeSet<String>> {
    fn walk(v: &Value, out: &mut BTreeSet<String>) {
        match v {
            Value::Str(s) => {
                if let Some(n) = s.strip_prefix("res://") {
                    out.insert(n.to_string());
                }
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            Value::Object(f) => f.iter().for_each(|(_, x)| walk(x, out)),
            Value::Map(m) => m.iter().for_each(|(_, x)| walk(x, out)),
            _ => {}
        }
    }
    let mut out = BTreeSet::new();
    for path in captures {
        let bytes = std::fs::read(path).with_context(|| path.display().to_string())?;
        let (records, _) = rekindle_wire::capture::parse(&bytes)
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        for r in records {
            let Ok(turn) = Turn::from_bytes(&r.bytes) else {
                continue;
            };
            for op in &turn.ops {
                if let NodeOp::Create { props, .. } | NodeOp::Update { props, .. } = op {
                    for (_, v) in props {
                        walk(v, &mut out);
                    }
                }
            }
        }
    }
    Ok(out)
}

/// The size in dp an SVG phoenix-view drew says it has: its `width` and
/// `height` are four times that.
fn svg_size(svg: &[u8]) -> Option<(f32, f32)> {
    let s = std::str::from_utf8(svg.get(..400.min(svg.len()))?).ok()?;
    let attr = |name: &str| -> Option<f32> {
        let at = s.find(&format!(" {name}=\""))? + name.len() + 3;
        s[at..].split('"').next()?.parse().ok()
    };
    Some((attr("width")? / 4., attr("height")? / 4.))
}

fn font_face(file: &str) -> Option<(&'static str, u16, bool)> {
    let stem = file.strip_suffix(".ttf")?;
    let (family, style) = stem.split_once('-')?;
    let family = match family {
        "ggsans" => "gg sans",
        "ggmono" => "gg mono",
        f if f.contains("Ginto") => "ABC Ginto Nord",
        _ => return None,
    };
    let italic = style.ends_with("Italic");
    let weight = match style.trim_end_matches("Italic") {
        "Medium" => 500,
        "Semibold" => 600,
        "Bold" => 700,
        "ExtraBold" => 800,
        "Black" => 900,
        _ => 400,
    };
    Some((family, weight, italic))
}

/// Write the assets for `captures` from `apk` into `dir`. `night` picks the
/// drawables' night configuration.
pub fn run(apk: &Apk, dir: &Path, captures: &[&Path], night: bool) -> Result<()> {
    std::fs::create_dir_all(dir.join("fonts"))?;
    let want = Want { dpi: 480, night };
    let mut drawables = Map::new();
    let mut missing = Vec::new();
    for name in names(captures)? {
        match drawable::load(apk, &name, want, None, None) {
            Ok(l) => {
                let mime = match l.format {
                    gpui::ImageFormat::Svg => "image/svg+xml",
                    gpui::ImageFormat::Jpeg => "image/jpeg",
                    gpui::ImageFormat::Webp => "image/webp",
                    gpui::ImageFormat::Gif => "image/gif",
                    _ => "image/png",
                };
                let uri = format!(
                    "data:{mime};base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(&l.bytes)
                );
                let size = (l.format == gpui::ImageFormat::Svg && !l.boxed)
                    .then(|| svg_size(&l.bytes))
                    .flatten();
                drawables.insert(
                    name,
                    json!({
                        "uri": uri,
                        "width": size.map(|s| s.0),
                        "height": size.map(|s| s.1),
                    }),
                );
            }
            Err(_) => missing.push(name),
        }
    }
    std::fs::write(
        dir.join("drawables.json"),
        Json::Object(drawables.clone()).to_string(),
    )?;
    let mut fonts = Vec::new();
    for name in apk.names().map(str::to_string).collect::<Vec<_>>() {
        let Some(file) = name.strip_prefix("assets/fonts/") else {
            continue;
        };
        let Some((family, weight, italic)) = font_face(file) else {
            continue;
        };
        if let Some(bytes) = apk.read(&name) {
            std::fs::write(dir.join("fonts").join(file), bytes)?;
            fonts.push(json!({"file": file, "family": family, "weight": weight, "italic": italic}));
        }
    }
    std::fs::write(
        dir.join("fonts.json"),
        Json::Array(fonts.clone()).to_string(),
    )?;
    println!(
        "phoenix-view: {} drawables and {} fonts in {}",
        drawables.len(),
        fonts.len(),
        dir.display()
    );
    if !missing.is_empty() {
        println!("phoenix-view: not in the APK: {}", missing.join(", "));
    }
    Ok(())
}
