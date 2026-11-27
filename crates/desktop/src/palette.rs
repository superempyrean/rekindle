//! The client's theme palette: every colour token of every theme, in one
//! table its components read from as they render.
//!
//! The client carries its tokens as a JSON file in the image, a theme family
//! `{"themes": [{"id", "name", "mode", "colors": {token: "#rrggbbaa"}}]}`.
//! On first use it parses the file once into a static array of themes in a
//! writable section, and `theme(id: u8) -> &Theme` returns one of them:
//!
//! ```text
//! mov eax, [rip + family + count * stride]   ; the OnceLock's state
//! test eax, eax
//! jne init
//! movzx eax, cl
//! imul rcx, rax, stride
//! lea rax, [rip + family]
//! add rax, rcx
//! ```
//!
//! Each theme holds its colours as packed `0xRRGGBBAA` words, one per token
//! in the file's (sorted) order. A component that draws with a token reads
//! the word through the theme it was given, so a word written into the
//! table reaches every element that uses the token, in every state.
//!
//! [`resolve`] finds the file and the getter, and checks one against the
//! other: the family ends where the getter's state word is, after as many
//! themes as the file has. Where the colours sit inside a theme is only
//! known once the client has parsed the file, so [`Layout::find`] reads it
//! from the live table by matching the file's values.

use std::collections::BTreeSet;

use anyhow::{Context, Result, bail};
use iced_x86::{Decoder, DecoderOptions, Mnemonic, OpKind, Register};
use serde::Serialize;

use crate::image::Image;

/// The client's palette, by virtual address.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Palette {
    /// `theme(id: u8) -> &Theme`.
    pub get: u64,
    /// The first theme of the family.
    pub family: u64,
    /// Bytes from one theme to the next.
    pub stride: u32,
    /// The token file.
    pub file: u64,
    pub file_len: u32,
}

/// One theme of the token file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileTheme {
    /// `dark`, `light`, `midnight`, ...
    pub id: String,
    pub name: String,
    /// `dark` or `light`.
    pub mode: String,
    /// Each token and its `0xRRGGBBAA`, in the file's order, which is the
    /// table's.
    pub colors: Vec<(String, u32)>,
}

/// The token file's themes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TokenFile {
    pub themes: Vec<FileTheme>,
}

impl TokenFile {
    /// Parse the file. Its colours are `#rrggbbaa` or `#rrggbb`.
    pub fn parse(text: &[u8]) -> Result<TokenFile> {
        let json: serde_json::Value = serde_json::Deserializer::from_slice(text)
            .into_iter()
            .next()
            .context("an empty token file")?
            .context("an unreadable token file")?;
        let themes = json
            .get("themes")
            .and_then(|t| t.as_array())
            .context("no `themes` array")?;
        let text_of = |t: &serde_json::Value, key: &str| {
            t.get(key)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let mut out = Vec::new();
        for t in themes {
            let colours = t
                .get("colors")
                .and_then(|c| c.as_object())
                .context("a theme without `colors`")?;
            let colors = colours
                .iter()
                .map(|(token, v)| {
                    let hex = v.as_str().and_then(|s| s.strip_prefix('#'));
                    let rgba = hex
                        .and_then(|h| u32::from_str_radix(h, 16).ok().map(|v| (h.len(), v)))
                        .and_then(|(len, v)| match len {
                            8 => Some(v),
                            6 => Some((v << 8) | 0xff),
                            _ => None,
                        })
                        .with_context(|| format!("{token}: not a colour: {v}"))?;
                    Ok((token.clone(), rgba))
                })
                .collect::<Result<Vec<_>>>()?;
            out.push(FileTheme {
                id: text_of(t, "id"),
                name: text_of(t, "name"),
                mode: text_of(t, "mode"),
                colors,
            });
        }
        if out.is_empty() {
            bail!("a token file without themes");
        }
        let tokens = |t: &FileTheme| t.colors.iter().map(|c| c.0.clone()).collect::<Vec<_>>();
        if out.iter().any(|t| tokens(t) != tokens(&out[0])) {
            bail!("the themes don't have the same tokens");
        }
        Ok(TokenFile { themes: out })
    }

    /// The tokens, in the table's order.
    #[must_use]
    pub fn tokens(&self) -> Vec<&str> {
        self.themes[0].colors.iter().map(|c| c.0.as_str()).collect()
    }
}

/// Where the colours are in each theme of the live family.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The offset of the first colour in a theme.
    pub colours_at: usize,
    /// For each theme of the family, the file's theme it was parsed from.
    pub themes: Vec<usize>,
}

impl Layout {
    /// Read the layout off the live family, `stride` bytes per theme: each
    /// theme must hold one of the file's themes' colours, all of them, at
    /// one offset shared by every theme. `None` before the client has
    /// parsed the file, or if a colour was changed since.
    #[must_use]
    pub fn find(family: &[u8], stride: usize, file: &TokenFile) -> Option<Layout> {
        let count = file.themes.len();
        let words = |t: &FileTheme| -> Vec<u8> {
            t.colors.iter().flat_map(|c| c.1.to_le_bytes()).collect()
        };
        let all: Vec<Vec<u8>> = file.themes.iter().map(words).collect();
        let len = all[0].len();
        if family.len() < count * stride || len > stride {
            return None;
        }
        let first = &family[..stride];
        let colours_at = (0..=stride - len)
            .step_by(4)
            .find(|&at| all.iter().any(|w| first[at..at + len] == w[..]))?;
        let themes = (0..count)
            .map(|slot| {
                let at = slot * stride + colours_at;
                all.iter().position(|w| family[at..at + len] == w[..])
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Layout { colours_at, themes })
    }
}

/// The token file: the one JSON document in the image with a `themes`
/// array of themes that each have `colors`.
fn token_file(image: &Image<'_>) -> Result<(u64, u32, TokenFile)> {
    let mut found = Vec::new();
    for hit in image.find(b"\"themes\": [") {
        // Back to the start of the printable run the document is in.
        let mut start = hit;
        while start > hit.saturating_sub(0x10000)
            && image
                .read(start - 1, 1)
                .is_some_and(|b| matches!(b[0], 0x20..0x7f | b'\n' | b'\r' | b'\t'))
        {
            start -= 1;
        }
        if image.read(start, 1) != Some(b"{") {
            continue;
        }
        // The document ends before the first byte that isn't printable.
        let mut len = 0usize;
        while let Some(chunk) = image.read(start + len as u64, 0x1000) {
            match chunk
                .iter()
                .position(|b| !matches!(b, 0x20..0x7f | b'\n' | b'\r' | b'\t'))
            {
                Some(at) => {
                    len += at;
                    break;
                }
                None => len += chunk.len(),
            }
        }
        let Some(text) = image.read(start, len) else {
            continue;
        };
        let Ok(file) = TokenFile::parse(text) else {
            continue;
        };
        let size = document_len(text).unwrap_or(len);
        found.push((start, u32::try_from(size)?, file));
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => bail!("no token file (a JSON theme family with `colors`)"),
        n => bail!("{n} token files"),
    }
}

/// The length of the JSON document at the start of `text`.
fn document_len(text: &[u8]) -> Option<usize> {
    let mut stream = serde_json::Deserializer::from_slice(text).into_iter::<serde_json::Value>();
    stream.next()?.ok()?;
    Some(stream.byte_offset())
}

/// Each `imul r64, r64, stride` directly followed by `lea r64, [rip +
/// family]` with `family` in a writable data section, as `(site, stride,
/// family)`.
fn indexings(image: &Image<'_>) -> Vec<(u64, u32, u64)> {
    let mut out = Vec::new();
    for (start, end) in image.text() {
        let Some(code) = image.read(start, (end - start) as usize) else {
            continue;
        };
        let mut at = 0;
        while at + 14 <= code.len() {
            // REX.W, 0x69, mod 11: `imul r64, r/m64, imm32`.
            if matches!(code[at], 0x48 | 0x49 | 0x4c | 0x4d)
                && code[at + 1] == 0x69
                && code[at + 2] >= 0xc0
                && matches!(code[at + 7], 0x48 | 0x4c)
                && code[at + 8] == 0x8d
                && code[at + 9] & 0xc7 == 0x05
            {
                let stride = u32::from_le_bytes(code[at + 3..at + 7].try_into().unwrap());
                let disp = i32::from_le_bytes(code[at + 10..at + 14].try_into().unwrap());
                let family = (start + at as u64 + 14).wrapping_add_signed(i64::from(disp));
                if stride >= 0x100 && image.section(family) == Some(".data") {
                    out.push((start + at as u64, stride, family));
                }
            }
            at += 1;
        }
    }
    out
}

/// Whether the function around `site` loads the dword at `state` into a
/// 32-bit register and tests it.
fn reads_state(image: &Image<'_>, site: u64, state: u64) -> Option<u64> {
    let (start, end) = image.function(site)?;
    let code = image.read(start, usize::try_from(end - start).ok()?)?;
    let insns: Vec<_> = Decoder::with_ip(64, code, start, DecoderOptions::NONE)
        .into_iter()
        .collect();
    insns
        .windows(2)
        .any(|w| {
            w[0].mnemonic() == Mnemonic::Mov
                && w[0].op0_kind() == OpKind::Register
                && w[0].op0_register().size() == 4
                && w[0].op1_kind() == OpKind::Memory
                && w[0].memory_base() == Register::RIP
                && w[0].ip_rel_memory_address() == state
                && w[1].mnemonic() == Mnemonic::Test
                && w[1].op0_register() == w[0].op0_register()
        })
        .then_some(start)
}

/// Find the token file and `theme(id)`.
pub fn resolve(image: &Image<'_>) -> Result<(Palette, TokenFile)> {
    let (file, file_len, tokens) = token_file(image)?;
    let count = tokens.themes.len() as u64;
    let words = tokens.themes[0].colors.len() as u32 * 4;
    let mut getters = BTreeSet::new();
    for (site, stride, family) in indexings(image) {
        if stride < words {
            continue;
        }
        let state = family + count * u64::from(stride);
        if let Some(get) = reads_state(image, site, state) {
            getters.insert((get, family, stride));
        }
    }
    let getters: Vec<_> = getters.into_iter().collect();
    let (get, family, stride) = crate::image::one_of(
        &format!("theme(id) over {count} themes of at least {words:#x} bytes"),
        &getters,
    )?;
    Ok((
        Palette {
            get,
            family,
            stride,
            file,
            file_len,
        },
        tokens,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r##"{
  "name": "t",
  "themes": [
    {"name": "Dark", "id": "dark", "mode": "dark",
     "colors": {"a": "#11223344", "b": "#556677ff", "c": "#000000"}},
    {"name": "Light", "id": "light", "mode": "light",
     "colors": {"a": "#ffffffff", "b": "#eeeeeeff", "c": "#dddddd80"}}
  ]
}
trailing bytes"##;

    #[test]
    fn a_token_file_parses_in_order() {
        let f = TokenFile::parse(FILE.as_bytes()).unwrap();
        assert_eq!(f.tokens(), ["a", "b", "c"]);
        assert_eq!(f.themes[0].colors[0].1, 0x11223344);
        assert_eq!(f.themes[0].colors[2].1, 0x000000ff);
        assert_eq!(f.themes[1].mode, "light");
        assert_eq!(
            document_len(FILE.as_bytes()),
            Some(FILE.find("\ntrailing").unwrap())
        );
    }

    #[test]
    fn the_layout_is_read_off_the_live_family() {
        let f = TokenFile::parse(FILE.as_bytes()).unwrap();
        let stride = 0x40;
        let mut family = vec![0u8; stride * 2];
        // Light first, then dark, each at +0x10.
        for (slot, theme) in [1usize, 0].into_iter().enumerate() {
            for (i, (_, v)) in f.themes[theme].colors.iter().enumerate() {
                let at = slot * stride + 0x10 + i * 4;
                family[at..at + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        let l = Layout::find(&family, stride, &f).unwrap();
        assert_eq!(l.colours_at, 0x10);
        assert_eq!(l.themes, [1, 0]);
        // A changed colour, or a family not yet parsed, has no layout.
        family[stride + 0x14] ^= 1;
        assert!(Layout::find(&family, stride, &f).is_none());
        assert!(Layout::find(&vec![0u8; stride * 2], stride, &f).is_none());
    }

    #[test]
    fn the_getter_indexes_the_family() {
        // Build 2208's `theme(id)`.
        let code = [
            0x53, 0x48, 0x83, 0xec, 0x20, 0x89, 0xcb, 0x8b, 0x05, 0xa3, 0xbf, 0xa3, 0x0c, 0x85,
            0xc0, 0x75, 0x1a, 0x0f, 0xb6, 0xc3, 0x48, 0x69, 0xc8, 0x20, 0x0c, 0x00, 0x00, 0x48,
            0x8d, 0x05, 0x0e, 0x8f, 0xa3, 0x0c,
        ];
        let insns: Vec<_> = Decoder::with_ip(64, &code, 0x14b448f00, DecoderOptions::NONE)
            .into_iter()
            .collect();
        let lea = insns.last().unwrap();
        assert_eq!(lea.ip_rel_memory_address(), 0x157e81e30);
        let state = insns[3].ip_rel_memory_address();
        assert_eq!(state, 0x157e81e30 + 4 * 0xc20);
    }

    /// Against the client on disk, when it is there.
    #[test]
    fn resolves_a_build_on_disk() {
        let Ok(path) = std::env::var("REKINDLE_DESKTOP_EXE") else {
            eprintln!("REKINDLE_DESKTOP_EXE not set; skipped");
            return;
        };
        let bytes = std::fs::read(path).unwrap();
        let image = Image::parse(&bytes).unwrap();
        let (p, file) = resolve(&image).unwrap();
        eprintln!("{p:#x?}");
        let rva = |va: u64| va - image.base();
        assert_eq!(rva(p.get), 0xb448f00);
        assert_eq!(rva(p.family), 0x17e81e30);
        assert_eq!(p.stride, 0xc20);
        assert_eq!(
            file.themes
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            ["dark", "light", "midnight", "darker"]
        );
        assert_eq!(file.tokens().len(), 749);
        assert_eq!(
            file.themes[3].colors[0],
            ("activity-timebar-progress-background".into(), 0x5865f2ff)
        );
    }
}
