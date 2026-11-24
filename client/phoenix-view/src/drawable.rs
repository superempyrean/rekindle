//! Android drawables from the APK, as GPUI images: a raster as it is, an
//! XML drawable converted to SVG for GPUI's resvg.
//!
//! The XML kinds drawn are `vector` (paths, groups, clip paths, gradient
//! fills), `shape` (rectangle, oval and line, with solid, gradient, stroke
//! and corners), `layer-list`, `inset`, `bitmap`, `color`, `ripple` (its
//! content), `adaptive-icon`, `animated-vector` (its first frame), `rotate`,
//! `scale` and `clip` (unrotated, unscaled and unclipped), `transition` (its
//! first layer) and the default item of `selector`, `animated-selector` and
//! `level-list`. A tint
//! is drawn as Android's `setTint` does, `SRC_IN`: the tint's colour where
//! the drawable has alpha.

use std::fmt::Write as _;

use base64::Engine as _;

use crate::apk::Apk;
use crate::axml::{self, Element, Val};
use crate::res::{Table, Value, Want};

/// A drawable ready for `gpui::Image::from_bytes`.
pub struct Loaded {
    pub format: gpui::ImageFormat,
    pub bytes: Vec<u8>,
    /// How it was found, for the log: the entry and its file.
    pub how: String,
    /// It has no size of its own and was drawn into the box asked for.
    pub boxed: bool,
}

/// Why a drawable couldn't be loaded.
#[derive(Debug)]
pub enum Missing {
    /// No entry by that name or its alias.
    NotInTable,
    /// An entry, but no configuration this device would get.
    NoConfig,
    /// It refers to a resource the APK set has no configuration of.
    Absent(String),
    /// Something in it this converter doesn't draw.
    Unsupported(String),
}

/// The framework colours drawables in an app refer to (`android.R.color`).
fn framework_color(id: u32) -> Option<u32> {
    match id {
        0x0106_000b => Some(0xffff_ffff),
        0x0106_000c => Some(0xff00_0000),
        0x0106_000d => Some(0),
        _ => None,
    }
}

/// A drawable, before it is drawn into a box.
enum Art {
    Raster {
        format: gpui::ImageFormat,
        bytes: Vec<u8>,
        /// Its size in dp.
        size: (f32, f32),
    },
    Vector {
        size: (f32, f32),
        viewport: (f32, f32),
        body: String,
    },
    Shape(Element),
    Color(u32),
    Layers(Vec<Layer>),
    /// Nothing to draw (an empty layer-list, a ripple's mask).
    Empty,
}

struct Layer {
    art: Art,
    /// left, top, right, bottom, in dp.
    inset: [f32; 4],
    size: (Option<f32>, Option<f32>),
    gravity: Option<u32>,
}

struct Ctx<'a> {
    apk: &'a Apk,
    table: &'a Table,
    want: Want,
    defs: String,
    next_id: usize,
    /// The density the files are at, to size rasters in dp.
    file: String,
}

fn num(v: Option<&Val>) -> Option<f32> {
    match v? {
        Val::Float(f) | Val::Dim(f, _) => Some(*f),
        Val::Int(i) => Some(*i as f32),
        _ => None,
    }
}

/// `#rrggbb` and an opacity, from ARGB.
fn rgb(c: u32) -> (String, f32) {
    (
        format!("#{:06x}", c & 0xff_ffff),
        ((c >> 24) & 0xff) as f32 / 255.0,
    )
}

fn data_uri(format: gpui::ImageFormat, bytes: &[u8]) -> String {
    let mime = match format {
        gpui::ImageFormat::Jpeg => "image/jpeg",
        gpui::ImageFormat::Webp => "image/webp",
        _ => "image/png",
    };
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// A PNG's or WebP's size in pixels.
fn pixel_size(format: gpui::ImageFormat, b: &[u8]) -> Option<(u32, u32)> {
    let be = |i: usize| Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?));
    let le16 = |i: usize| {
        Some(u32::from(u16::from_le_bytes(
            b.get(i..i + 2)?.try_into().ok()?,
        )))
    };
    let le24 = |i: usize| {
        let s = b.get(i..i + 3)?;
        Some(u32::from(s[0]) | u32::from(s[1]) << 8 | u32::from(s[2]) << 16)
    };
    match format {
        gpui::ImageFormat::Png => Some((be(16)?, be(20)?)),
        gpui::ImageFormat::Webp => match b.get(12..16)? {
            b"VP8 " => Some((le16(26)? & 0x3fff, le16(28)? & 0x3fff)),
            b"VP8L" => {
                let bits = u32::from_le_bytes(b.get(21..25)?.try_into().ok()?);
                Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
            }
            b"VP8X" => Some((le24(24)? + 1, le24(27)? + 1)),
            _ => None,
        },
        _ => None,
    }
}

/// The dpi a resource directory's name gives its files.
fn dir_dpi(path: &str) -> f32 {
    const DPI: [(&str, f32); 7] = [
        ("-xxxhdpi", 640.),
        ("-xxhdpi", 480.),
        ("-xhdpi", 320.),
        ("-hdpi", 240.),
        ("-mdpi", 160.),
        ("-ldpi", 120.),
        ("-tvdpi", 213.),
    ];
    DPI.iter()
        .find(|(q, _)| path.contains(q))
        .map_or(160., |(_, d)| *d)
}

/// Android's `Gravity` bits.
mod gravity {
    pub const CENTER_H: u32 = 0x01;
    pub const LEFT: u32 = 0x03;
    pub const RIGHT: u32 = 0x05;
    pub const CENTER_V: u32 = 0x10;
    pub const TOP: u32 = 0x30;
    pub const BOTTOM: u32 = 0x50;
    pub const FILL_H: u32 = 0x07;
    pub const FILL_V: u32 = 0x70;
}

/// Load the drawable `name` (`res://<name>`) for `want`, tinted with `tint`
/// (ARGB), drawn into a box of `size` dp when it has no size of its own.
pub fn load(
    apk: &Apk,
    name: &str,
    want: Want,
    tint: Option<u32>,
    size: Option<(f32, f32)>,
) -> Result<Loaded, Missing> {
    let table = apk.table().ok_or(Missing::NotInTable)?;
    let id = table
        .id("drawable", name)
        .or_else(|| table.id("mipmap", name))
        .ok_or(Missing::NotInTable)?;
    let mut cx = Ctx {
        apk,
        table,
        want,
        defs: String::new(),
        next_id: 0,
        file: String::new(),
    };
    let art = cx.by_id(id)?;
    let how = format!("{} ({})", table.name(id), cx.file);
    if let (Art::Raster { format, bytes, .. }, None) = (&art, tint) {
        return Ok(Loaded {
            format: *format,
            bytes: bytes.clone(),
            how,
            boxed: false,
        });
    }
    let intrinsic = Ctx::intrinsic(&art);
    let (w, h) = intrinsic.or(size).unwrap_or((24., 24.));
    let mut body = String::new();
    cx.draw(&art, 0., 0., w, h, &mut body);
    if let Some(t) = tint {
        let f = cx.tint_filter(t);
        body = format!("<g filter=\"url(#{f})\">{body}</g>");
    }
    // Drawn at four times its size in dp, so it stays sharp when GPUI
    // rasterises it at the window's scale.
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {w} {h}\"><defs>{}</defs>{body}</svg>",
        w * 4.,
        h * 4.,
        cx.defs
    );
    Ok(Loaded {
        format: gpui::ImageFormat::Svg,
        bytes: svg.into_bytes(),
        boxed: intrinsic.is_none(),
        how: format!(
            "{how}, as SVG{}",
            if tint.is_some() { ", tinted" } else { "" }
        ),
    })
}

/// Why `name` isn't in the table, as far as the APK set tells, and
/// whether the set lacks the density split a Play install of its base has
/// (the split holds the density-specific drawables).
pub fn explain_missing(apk: &Apk, name: &str) -> (String, bool) {
    let alias = apk
        .table()
        .and_then(|t| t.alias("drawable", name))
        .map(|a| format!(" (nor its alias {a})"))
        .unwrap_or_default();
    let splits = apk.splits();
    let loaded: Vec<String> = splits
        .iter()
        .map(|(_, s)| s.clone().unwrap_or_else(|| "base".into()))
        .collect();
    let density = splits
        .iter()
        .any(|(_, s)| s.as_deref().is_some_and(|s| s.ends_with("dpi")));
    let incomplete = !density && apk.required_splits().iter().any(|r| r == "base__density");
    let hint = if incomplete {
        "; the base requires a density split (base__density) and none is loaded: put the \
         device's split_config.<density>.apk beside the base, or pass it with --split"
    } else {
        ""
    };
    (
        format!(
            "no drawable named {name}{alias} in the APK set [{}]{hint}",
            loaded.join(", ")
        ),
        incomplete,
    )
}

impl Ctx<'_> {
    fn id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }

    fn by_id(&mut self, id: u32) -> Result<Art, Missing> {
        match self.table.best(id, self.want) {
            Some(Value::Val(v)) => {
                let v = v.clone();
                self.by_value(&v)
            }
            Some(Value::Bag) => Err(Missing::Unsupported(format!(
                "{} is a complex entry",
                self.table.name(id)
            ))),
            None => Err(Missing::NoConfig),
        }
    }

    fn by_value(&mut self, v: &Val) -> Result<Art, Missing> {
        match self.table.resolve(v, self.want) {
            Val::Str(path) => self.file_art(&path),
            Val::Color(c) => Ok(Art::Color(c)),
            Val::Attr(a) => Err(Missing::Unsupported(format!(
                "theme attribute {}",
                self.table.name(a)
            ))),
            Val::Null => Ok(Art::Empty),
            Val::Ref(id) => match framework_color(id) {
                Some(c) => Ok(Art::Color(c)),
                None => Err(Missing::Absent(self.table.name(id))),
            },
            other => Err(Missing::Unsupported(format!("a drawable value {other:?}"))),
        }
    }

    fn file_art(&mut self, path: &str) -> Result<Art, Missing> {
        let bytes = self
            .apk
            .read(path)
            .ok_or_else(|| Missing::Unsupported(format!("{path} is not in the APK")))?;
        if self.file.is_empty() {
            self.file = path.to_string();
        }
        let format = match path.rsplit('.').next() {
            Some("png") => Some(gpui::ImageFormat::Png),
            Some("webp") => Some(gpui::ImageFormat::Webp),
            Some("jpg" | "jpeg") => Some(gpui::ImageFormat::Jpeg),
            _ => None,
        };
        if let Some(format) = format {
            let (pw, ph) = pixel_size(format, &bytes).unwrap_or((24, 24));
            let k = dir_dpi(path) / 160.;
            return Ok(Art::Raster {
                format,
                bytes,
                size: (pw as f32 / k, ph as f32 / k),
            });
        }
        let el = axml::parse(&bytes).map_err(|e| Missing::Unsupported(format!("{path}: {e}")))?;
        self.element(&el).map_err(|m| match m {
            Missing::Unsupported(why) => Missing::Unsupported(format!("{path}: {why}")),
            m => m,
        })
    }

    /// A drawable an element names with `attr`, or its first child element.
    fn child_art(&mut self, el: &Element, attr: &str) -> Result<Art, Missing> {
        match el.get(attr) {
            Some(v) => {
                let v = v.clone();
                self.by_value(&v)
            }
            None => match el.children.first() {
                Some(c) => self.element(c),
                None => Ok(Art::Empty),
            },
        }
    }

    fn element(&mut self, el: &Element) -> Result<Art, Missing> {
        match el.name.as_str() {
            "vector" => self.vector(el),
            "shape" => Ok(Art::Shape(el.clone())),
            "color" => match el.get("color").map(|v| self.table.resolve(v, self.want)) {
                Some(Val::Color(c)) => Ok(Art::Color(c)),
                other => Err(Missing::Unsupported(format!("<color> of {other:?}"))),
            },
            "bitmap" | "nine-patch" => self.child_art(el, "src"),
            "animated-vector" | "rotate" | "scale" | "clip" => self.child_art(el, "drawable"),
            // Its first layer, before the transition runs.
            "transition" => match el.children.first() {
                Some(item) => self.child_art(item, "drawable"),
                None => Ok(Art::Empty),
            },
            "inset" => {
                let art = self.child_art(el, "drawable")?;
                let all = num(el.get("inset")).unwrap_or(0.);
                let side = |n: &str| num(el.get(n)).unwrap_or(all);
                Ok(Art::Layers(vec![Layer {
                    art,
                    inset: [
                        side("insetLeft"),
                        side("insetTop"),
                        side("insetRight"),
                        side("insetBottom"),
                    ],
                    size: (None, None),
                    gravity: None,
                }]))
            }
            "layer-list" | "ripple" | "adaptive-icon" => {
                let mut layers = Vec::new();
                for item in &el.children {
                    if el.name == "ripple"
                        && matches!(item.get("id"), Some(Val::Ref(id)) if *id == 0x0102_002e)
                    {
                        // android:id/mask: where the ripple shows, not drawn.
                        continue;
                    }
                    let art = self.child_art(item, "drawable")?;
                    let side = |n: &str| num(item.get(n)).unwrap_or(0.);
                    layers.push(Layer {
                        art,
                        inset: [side("left"), side("top"), side("right"), side("bottom")],
                        size: (num(item.get("width")), num(item.get("height"))),
                        gravity: match item.get("gravity") {
                            Some(Val::Int(g)) => Some(*g as u32),
                            _ => None,
                        },
                    });
                }
                Ok(Art::Layers(layers))
            }
            "selector" | "animated-selector" | "level-list" => {
                // The item for no state: the first without a state_ or
                // level attribute, else the last.
                let item = el
                    .children
                    .iter()
                    .filter(|c| c.name == "item")
                    .find(|c| {
                        !c.attrs
                            .iter()
                            .any(|a| a.name.starts_with("state_") || a.name.ends_with("Level"))
                    })
                    .or_else(|| el.children.iter().rfind(|c| c.name == "item"));
                match item {
                    Some(item) => self.child_art(item, "drawable"),
                    None => Ok(Art::Empty),
                }
            }
            other => Err(Missing::Unsupported(format!("<{other}>"))),
        }
    }

    fn intrinsic(art: &Art) -> Option<(f32, f32)> {
        match art {
            Art::Raster { size, .. } | Art::Vector { size, .. } => Some(*size),
            Art::Shape(el) => {
                let s = el.children.iter().find(|c| c.name == "size")?;
                Some((num(s.get("width"))?, num(s.get("height"))?))
            }
            Art::Layers(layers) => layers.iter().find_map(|l| {
                let (w, h) = match l.size {
                    (Some(w), Some(h)) => (w, h),
                    _ => Self::intrinsic(&l.art)?,
                };
                Some((w + l.inset[0] + l.inset[2], h + l.inset[1] + l.inset[3]))
            }),
            Art::Color(_) | Art::Empty => None,
        }
    }

    /// Draw `art` into the box at (`x`, `y`), `w` x `h` dp.
    fn draw(&mut self, art: &Art, x: f32, y: f32, w: f32, h: f32, out: &mut String) {
        match art {
            Art::Raster { format, bytes, .. } => {
                let _ = write!(
                    out,
                    "<image x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" preserveAspectRatio=\"none\" xlink:href=\"{}\"/>",
                    data_uri(*format, bytes)
                );
            }
            Art::Vector { viewport, body, .. } => {
                let _ = write!(
                    out,
                    "<svg x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {} {}\" preserveAspectRatio=\"none\" overflow=\"hidden\">{body}</svg>",
                    viewport.0, viewport.1
                );
            }
            Art::Color(c) => {
                let (fill, a) = rgb(*c);
                let _ = write!(
                    out,
                    "<rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" fill=\"{fill}\" fill-opacity=\"{a}\"/>"
                );
            }
            Art::Shape(el) => self.shape(el, x, y, w, h, out),
            Art::Layers(layers) => {
                for l in layers {
                    let (bx, by) = (x + l.inset[0], y + l.inset[1]);
                    let (bw, bh) = (
                        (w - l.inset[0] - l.inset[2]).max(0.),
                        (h - l.inset[1] - l.inset[3]).max(0.),
                    );
                    let (lw, lh) = (l.size.0.unwrap_or(bw), l.size.1.unwrap_or(bh));
                    // A layer with a size and no gravity sits top left;
                    // without a size it fills.
                    let g = l.gravity.unwrap_or(if l.size == (None, None) {
                        gravity::FILL_H | gravity::FILL_V
                    } else {
                        gravity::LEFT | gravity::TOP
                    });
                    let (lx, lw) = match g & 0x07 {
                        gravity::FILL_H => (bx, bw),
                        gravity::RIGHT => (bx + bw - lw, lw),
                        gravity::CENTER_H => (bx + (bw - lw) / 2., lw),
                        _ => (bx, lw),
                    };
                    let (ly, lh) = match g & 0x70 {
                        gravity::FILL_V => (by, bh),
                        gravity::BOTTOM => (by + bh - lh, lh),
                        gravity::CENTER_V => (by + (bh - lh) / 2., lh),
                        _ => (by, lh),
                    };
                    self.draw(&l.art, lx, ly, lw, lh, out);
                }
            }
            Art::Empty => {}
        }
    }

    /// An `feFlood` + `feComposite in` filter: Android's `SRC_IN` tint.
    fn tint_filter(&mut self, tint: u32) -> String {
        let id = self.id("tint");
        let (c, a) = rgb(tint);
        let _ = write!(
            self.defs,
            "<filter id=\"{id}\" color-interpolation-filters=\"sRGB\"><feFlood flood-color=\"{c}\" flood-opacity=\"{a}\"/><feComposite in2=\"SourceGraphic\" operator=\"in\"/></filter>"
        );
        id
    }

    /// A colour attribute: a colour, a colour state list's default, or a
    /// gradient (`<gradient>` in its own file, as aapt compiles
    /// `<aapt:attr>`). Returns an SVG paint and its opacity.
    fn paint(&mut self, v: Option<&Val>) -> Result<Option<(String, f32)>, Missing> {
        let Some(v) = v else { return Ok(None) };
        match self.table.resolve(v, self.want) {
            Val::Color(c) => Ok(Some(rgb(c))),
            Val::Str(path)
                if std::path::Path::new(&path)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("xml")) =>
            {
                let el = self
                    .apk
                    .read(&path)
                    .and_then(|b| axml::parse(&b).ok())
                    .ok_or_else(|| Missing::Unsupported(format!("colour file {path}")))?;
                match el.name.as_str() {
                    "selector" => {
                        let item = el
                            .children
                            .iter()
                            .find(|c| !c.attrs.iter().any(|a| a.name.starts_with("state_")))
                            .or(el.children.last());
                        let Some(item) = item else { return Ok(None) };
                        let base = self.paint(item.get("color"))?;
                        let alpha = num(item.get("alpha")).unwrap_or(1.);
                        Ok(base.map(|(c, a)| (c, a * alpha)))
                    }
                    "gradient" => Ok(Some((format!("url(#{})", self.vector_gradient(&el)?), 1.))),
                    other => Err(Missing::Unsupported(format!("<{other}> as a colour"))),
                }
            }
            Val::Null => Ok(None),
            Val::Ref(id) => match framework_color(id) {
                Some(c) => Ok(Some(rgb(c))),
                None => Err(Missing::Absent(self.table.name(id))),
            },
            Val::Attr(a) => Err(Missing::Unsupported(format!(
                "theme attribute {}",
                self.table.name(a)
            ))),
            other => Err(Missing::Unsupported(format!("a colour {other:?}"))),
        }
    }

    fn stops(&mut self, el: &Element, out: &mut String) -> Result<(), Missing> {
        let items: Vec<&Element> = el.children.iter().filter(|c| c.name == "item").collect();
        if items.is_empty() {
            let mut cs = vec![(0., el.get("startColor"))];
            if el.get("centerColor").is_some() {
                cs.push((0.5, el.get("centerColor")));
            }
            cs.push((1., el.get("endColor")));
            for (o, c) in cs {
                let (c, a) = self.paint(c)?.unwrap_or(("#000000".into(), 0.));
                let _ = write!(
                    out,
                    "<stop offset=\"{o}\" stop-color=\"{c}\" stop-opacity=\"{a}\"/>"
                );
            }
        } else {
            for it in items {
                let o = num(it.get("offset")).unwrap_or(0.);
                let (c, a) = self
                    .paint(it.get("color"))?
                    .unwrap_or(("#000000".into(), 0.));
                let _ = write!(
                    out,
                    "<stop offset=\"{o}\" stop-color=\"{c}\" stop-opacity=\"{a}\"/>"
                );
            }
        }
        Ok(())
    }

    /// A vector's `<gradient>`, in viewport coordinates.
    fn vector_gradient(&mut self, el: &Element) -> Result<String, Missing> {
        let id = self.id("grad");
        let f = |n: &str| num(el.get(n)).unwrap_or(0.);
        let spread = match el.get("tileMode") {
            Some(Val::Int(1)) => " spreadMethod=\"repeat\"",
            Some(Val::Int(2)) => " spreadMethod=\"reflect\"",
            _ => "",
        };
        let mut stops = String::new();
        self.stops(el, &mut stops)?;
        let def = match el.get("type") {
            Some(Val::Int(1)) => format!(
                "<radialGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\" cx=\"{}\" cy=\"{}\" r=\"{}\"{spread}>{stops}</radialGradient>",
                f("centerX"),
                f("centerY"),
                f("gradientRadius")
            ),
            Some(Val::Int(2)) => {
                return Err(Missing::Unsupported("a sweep gradient".into()));
            }
            _ => format!(
                "<linearGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"{spread}>{stops}</linearGradient>",
                f("startX"),
                f("startY"),
                f("endX"),
                f("endY")
            ),
        };
        self.defs.push_str(&def);
        Ok(id)
    }

    fn vector(&mut self, el: &Element) -> Result<Art, Missing> {
        let w = num(el.get("width")).unwrap_or(24.);
        let h = num(el.get("height")).unwrap_or(24.);
        let vw = num(el.get("viewportWidth")).unwrap_or(w);
        let vh = num(el.get("viewportHeight")).unwrap_or(h);
        let mut body = String::new();
        self.group(el, &mut body)?;
        if let Some(a) = num(el.get("alpha")).filter(|a| *a < 1.) {
            body = format!("<g opacity=\"{a}\">{body}</g>");
        }
        if let Some((c, a)) = self.paint(el.get("tint"))? {
            let id = self.id("tint");
            let _ = write!(
                self.defs,
                "<filter id=\"{id}\" color-interpolation-filters=\"sRGB\"><feFlood flood-color=\"{c}\" flood-opacity=\"{a}\"/><feComposite in2=\"SourceGraphic\" operator=\"in\"/></filter>"
            );
            body = format!("<g filter=\"url(#{id})\">{body}</g>");
        }
        Ok(Art::Vector {
            size: (w, h),
            viewport: (vw, vh),
            body,
        })
    }

    /// A `<vector>`'s or `<group>`'s children. A `<clip-path>` clips the
    /// siblings after it.
    fn group(&mut self, el: &Element, out: &mut String) -> Result<(), Missing> {
        let mut open = 0;
        for c in &el.children {
            match c.name.as_str() {
                "path" => self.path(c, out)?,
                "group" => {
                    let f = |n: &str, d: f32| num(c.get(n)).unwrap_or(d);
                    let (px, py) = (f("pivotX", 0.), f("pivotY", 0.));
                    let t = format!(
                        "translate({} {}) rotate({}) scale({} {}) translate({} {})",
                        px + f("translateX", 0.),
                        py + f("translateY", 0.),
                        f("rotation", 0.),
                        f("scaleX", 1.),
                        f("scaleY", 1.),
                        -px,
                        -py
                    );
                    let _ = write!(out, "<g transform=\"{t}\">");
                    self.group(c, out)?;
                    out.push_str("</g>");
                }
                "clip-path" => {
                    let id = self.id("clip");
                    let d = c.attr("pathData").unwrap_or_default();
                    let _ = write!(
                        self.defs,
                        "<clipPath id=\"{id}\"><path d=\"{d}\"/></clipPath>"
                    );
                    let _ = write!(out, "<g clip-path=\"url(#{id})\">");
                    open += 1;
                }
                other => return Err(Missing::Unsupported(format!("<{other}> in a vector"))),
            }
        }
        for _ in 0..open {
            out.push_str("</g>");
        }
        Ok(())
    }

    fn path(&mut self, el: &Element, out: &mut String) -> Result<(), Missing> {
        let d = el.attr("pathData").unwrap_or_default();
        let fill = self.paint(el.get("fillColor"))?;
        let stroke = self.paint(el.get("strokeColor"))?;
        let trim = (
            num(el.get("trimPathStart")).unwrap_or(0.),
            num(el.get("trimPathEnd")).unwrap_or(1.),
        );
        if trim.1 <= trim.0 {
            return Ok(());
        }
        if trim != (0., 1.) {
            return Err(Missing::Unsupported("a trimmed path".into()));
        }
        let _ = write!(out, "<path d=\"{d}\"");
        match fill {
            Some((c, a)) => {
                let a = a * num(el.get("fillAlpha")).unwrap_or(1.);
                let _ = write!(out, " fill=\"{c}\" fill-opacity=\"{a}\"");
                if matches!(el.get("fillType"), Some(Val::Int(1))) {
                    out.push_str(" fill-rule=\"evenodd\"");
                }
            }
            None => out.push_str(" fill=\"none\""),
        }
        if let Some((c, a)) = stroke {
            let a = a * num(el.get("strokeAlpha")).unwrap_or(1.);
            let w = num(el.get("strokeWidth")).unwrap_or(0.);
            let cap = match el.get("strokeLineCap") {
                Some(Val::Int(1)) => "round",
                Some(Val::Int(2)) => "square",
                _ => "butt",
            };
            let join = match el.get("strokeLineJoin") {
                Some(Val::Int(1)) => "round",
                Some(Val::Int(2)) => "bevel",
                _ => "miter",
            };
            let _ = write!(
                out,
                " stroke=\"{c}\" stroke-opacity=\"{a}\" stroke-width=\"{w}\" stroke-linecap=\"{cap}\" stroke-linejoin=\"{join}\" stroke-miterlimit=\"{}\"",
                num(el.get("strokeMiterLimit")).unwrap_or(4.)
            );
        }
        out.push_str("/>");
        Ok(())
    }

    /// A `<shape>` in the box. A part that doesn't convert is left out.
    fn shape(&mut self, el: &Element, x: f32, y: f32, w: f32, h: f32, out: &mut String) {
        let kid = |n: &str| el.children.iter().find(|c| c.name == n);
        let kind = match el.get("shape") {
            Some(Val::Int(k)) => *k,
            _ => 0,
        };
        if kind == 3 {
            // A ring: not drawn.
            return;
        }
        let fill = if let Some(g) = kid("gradient") {
            self.shape_gradient(g, x, y, w, h)
                .ok()
                .map(|id| (format!("url(#{id})"), 1.))
        } else {
            kid("solid").and_then(|s| self.paint(s.get("color")).ok().flatten())
        };
        let stroke = kid("stroke").and_then(|s| {
            let (c, a) = self.paint(s.get("color")).ok().flatten()?;
            let mut attrs = format!(
                " stroke=\"{c}\" stroke-opacity=\"{a}\" stroke-width=\"{}\"",
                num(s.get("width")).unwrap_or(0.)
            );
            if let (Some(dw), Some(dg)) = (num(s.get("dashWidth")), num(s.get("dashGap")))
                && dw > 0.
            {
                let _ = write!(attrs, " stroke-dasharray=\"{dw} {dg}\"");
            }
            Some((attrs, num(s.get("width")).unwrap_or(0.)))
        });
        let (fill_attr, sw) = (
            fill.map_or_else(
                || " fill=\"none\"".to_string(),
                |(c, a)| format!(" fill=\"{c}\" fill-opacity=\"{a}\""),
            ),
            stroke.as_ref().map_or(0., |s| s.1),
        );
        let stroke_attr = stroke.map(|s| s.0).unwrap_or_default();
        // The stroke is inside the bounds, as Android draws it.
        let (x, y, w, h) = (x + sw / 2., y + sw / 2., (w - sw).max(0.), (h - sw).max(0.));
        match kind {
            1 => {
                let _ = write!(
                    out,
                    "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\"{fill_attr}{stroke_attr}/>",
                    x + w / 2.,
                    y + h / 2.,
                    w / 2.,
                    h / 2.
                );
            }
            2 => {
                let _ = write!(
                    out,
                    "<line x1=\"{x}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" fill=\"none\"{stroke_attr}/>",
                    y + h / 2.,
                    x + w,
                    y + h / 2.
                );
            }
            _ => {
                let c = kid("corners");
                let all = c.and_then(|c| num(c.get("radius"))).unwrap_or(0.);
                let r = |n: &str| c.and_then(|c| num(c.get(n))).unwrap_or(all);
                let max = w.min(h) / 2.;
                let [tl, tr, br, bl] = [
                    r("topLeftRadius"),
                    r("topRightRadius"),
                    r("bottomRightRadius"),
                    r("bottomLeftRadius"),
                ]
                .map(|v| v.min(max));
                let _ = write!(
                    out,
                    "<path d=\"M{} {y}H{}A{tr} {tr} 0 0 1 {} {}V{}A{br} {br} 0 0 1 {} {}H{}A{bl} {bl} 0 0 1 {x} {}V{}A{tl} {tl} 0 0 1 {} {y}Z\"{fill_attr}{stroke_attr}/>",
                    x + tl,
                    x + w - tr,
                    x + w,
                    y + tr,
                    y + h - br,
                    x + w - br,
                    y + h,
                    x + bl,
                    y + h - bl,
                    y + tl,
                    x + tl,
                );
            }
        }
    }

    /// A `<shape>`'s `<gradient>`: `angle` 0 runs left to right, 90 bottom
    /// to top; `centerX`/`centerY` and a `%` radius are of the box.
    fn shape_gradient(
        &mut self,
        g: &Element,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    ) -> Result<String, Missing> {
        let id = self.id("grad");
        let mut stops = String::new();
        self.stops(g, &mut stops)?;
        let cx = x + w * num(g.get("centerX")).unwrap_or(0.5);
        let cy = y + h * num(g.get("centerY")).unwrap_or(0.5);
        let def = match g.get("type") {
            Some(Val::Int(1)) => {
                let r = match g.get("gradientRadius") {
                    Some(Val::Frac(f, _)) => f * w.min(h),
                    v => num(v).unwrap_or(w.min(h) / 2.),
                };
                format!(
                    "<radialGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\" cx=\"{cx}\" cy=\"{cy}\" r=\"{r}\">{stops}</radialGradient>"
                )
            }
            Some(Val::Int(2)) => return Err(Missing::Unsupported("a sweep gradient".into())),
            _ => {
                let angle = num(g.get("angle")).unwrap_or(0.).rem_euclid(360.);
                let (s, c) = angle.to_radians().sin_cos();
                // From the box's edge to the opposite one, through its
                // centre.
                let half = f32::midpoint(w * c.abs(), h * s.abs());
                let (mx, my) = (x + w / 2., y + h / 2.);
                format!(
                    "<linearGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\">{stops}</linearGradient>",
                    mx - c * half,
                    my + s * half,
                    mx + c * half,
                    my - s * half
                )
            }
        };
        self.defs.push_str(&def);
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use super::*;

    pub const BASE: &str = r"H:\phoenix-apk\discord-356207-base.apk";

    fn apk() -> Option<Apk> {
        let base = std::env::var("PHOENIX_APK").unwrap_or_else(|_| BASE.into());
        Path::new(&base)
            .exists()
            .then(|| Apk::open(Path::new(&base), &[]).expect("the APK opens"))
    }

    fn render(svg: &[u8]) -> resvg::tiny_skia::Pixmap {
        let tree = resvg::usvg::Tree::from_data(svg, &resvg::usvg::Options::default())
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(svg)));
        let size = tree.size().to_int_size();
        let mut pm = resvg::tiny_skia::Pixmap::new(size.width(), size.height()).unwrap();
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::default(),
            &mut pm.as_mut(),
        );
        pm
    }

    /// Every drawable in the APK set: each one loads, or the reason it
    /// doesn't is one this converter names; every SVG parses and paints.
    #[test]
    fn every_drawable_in_the_apk() {
        let Some(apk) = apk() else { return };
        let table = apk.table().expect("a resource table");
        let want = Want {
            dpi: 200,
            night: true,
        };
        let names: Vec<String> = table.names_of("drawable");
        let (mut raster, mut svg, mut why) = (0, 0, BTreeMap::<String, Vec<String>>::new());
        for name in &names {
            match load(&apk, name, want, None, Some((48., 48.))) {
                Ok(l) if l.format == gpui::ImageFormat::Svg => {
                    let pm = render(&l.bytes);
                    assert!(pm.width() > 0, "{name}");
                    svg += 1;
                }
                Ok(_) => raster += 1,
                Err(Missing::Unsupported(w)) => {
                    let kind = w.split(": ").last().unwrap_or(&w).to_string();
                    why.entry(kind).or_default().push(name.clone());
                }
                Err(m) => why.entry(format!("{m:?}")).or_default().push(name.clone()),
            }
        }
        let skipped: usize = why.values().map(Vec::len).sum();
        eprintln!(
            "{} drawables: {raster} raster, {svg} as SVG, {skipped} not drawn",
            names.len()
        );
        for (w, ns) in &why {
            eprintln!(
                "  {w}: {} ({})",
                ns.len(),
                ns.iter().take(4).cloned().collect::<Vec<_>>().join(", ")
            );
        }
        assert!(svg > 0);
    }

    /// The welcome and login screens' images come from the density splits,
    /// at the bucket the window's density asks for.
    #[test]
    fn the_density_splits_supply_the_auth_screens() {
        let Some(apk) = apk() else { return };
        let loaded: Vec<String> = apk.splits().into_iter().filter_map(|(_, s)| s).collect();
        if !["config.hdpi", "config.xxhdpi"]
            .iter()
            .all(|s| loaded.iter().any(|l| l == s))
        {
            return;
        }
        for (dpi, dir) in [(200, "drawable-xhdpi-v4/"), (480, "drawable-xxhdpi-v4/")] {
            let want = Want { dpi, night: true };
            for name in [
                "discord_logo",
                "register_background_dark",
                "ic_mana_arrow_large_left",
                "ic_mana_eye",
            ] {
                let l = load(&apk, name, want, None, None)
                    .unwrap_or_else(|e| panic!("{name} at {dpi} dpi: {e:?}"));
                assert!(l.how.contains(dir), "{name} at {dpi} dpi: {}", l.how);
            }
        }
    }

    /// A split of another build is left out.
    #[test]
    fn another_builds_split_is_left_out() {
        let other = Path::new(r"H:\phoenix-apk\357200\discord-357200-config.xxhdpi.apk");
        if !Path::new(BASE).exists() || !other.exists() {
            return;
        }
        let apk = Apk::open(Path::new(BASE), &[other.to_path_buf()]).expect("the APK opens");
        assert!(apk.skipped.iter().any(|(p, _)| p == other));
        assert!(apk.splits().iter().all(|(p, _)| p != other));
    }

    /// A tint paints the drawable's shape in the tint's colour.
    #[test]
    fn a_tint_is_src_in() {
        let Some(apk) = apk() else { return };
        let want = Want {
            dpi: 200,
            night: true,
        };
        let l = load(&apk, "ic_mana_eye_plus", want, Some(0xff00_ff00), None).expect("loads");
        let pm = render(&l.bytes);
        let painted: Vec<_> = pm.pixels().iter().filter(|p| p.alpha() == 255).collect();
        assert!(!painted.is_empty());
        assert!(
            painted
                .iter()
                .all(|p| p.red() == 0 && p.green() == 255 && p.blue() == 0)
        );
        if let Ok(dir) = std::env::var("PHOENIX_VIEW_DRAWABLES") {
            // On grey, to look at: PHOENIX_VIEW_DRAWABLES=<dir>[;name,name...].
            let (dir, names) = dir.split_once(';').unwrap_or((&dir, "ic_mana_eye_plus"));
            for name in names.split(',') {
                if let Ok(l) = load(&apk, name, want, None, Some((96., 96.))) {
                    let out = Path::new(dir).join(format!("{name}.png"));
                    if l.format == gpui::ImageFormat::Svg {
                        let pm = render(&l.bytes);
                        let mut bg =
                            resvg::tiny_skia::Pixmap::new(pm.width(), pm.height()).unwrap();
                        bg.fill(resvg::tiny_skia::Color::from_rgba8(128, 128, 128, 255));
                        bg.draw_pixmap(
                            0,
                            0,
                            pm.as_ref(),
                            &resvg::tiny_skia::PixmapPaint::default(),
                            resvg::tiny_skia::Transform::identity(),
                            None,
                        );
                        bg.save_png(&out).unwrap();
                    } else {
                        std::fs::write(&out, &l.bytes).unwrap();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod dump {
    /// `PHOENIX_VIEW_DUMP=<name> cargo test dump_one -- --ignored --nocapture`
    /// prints a drawable's XML tree and its SVG.
    #[test]
    #[ignore = "a tool"]
    fn dump_one() {
        let name = std::env::var("PHOENIX_VIEW_DUMP").expect("PHOENIX_VIEW_DUMP");
        let apk = super::Apk::open(std::path::Path::new(super::tests::BASE), &[]).unwrap();
        let t = apk.table().unwrap();
        let id = t.id("drawable", &name).expect("a drawable");
        let want = super::Want {
            dpi: 200,
            night: true,
        };
        if let Some(super::Value::Val(super::Val::Str(p))) = t.best(id, want)
            && let Some(el) = apk.read(p).and_then(|b| super::axml::parse(&b).ok())
        {
            eprintln!("{p}: {el:#?}");
        }
        match super::load(&apk, &name, want, None, Some((96., 96.))) {
            Ok(l) => eprintln!("{}", String::from_utf8_lossy(&l.bytes)),
            Err(e) => eprintln!("{e:?}"),
        }
    }
}
