//! Android's compiled binary XML (`res/*.xml` and `AndroidManifest.xml` in
//! an APK) and the typed values it shares with `resources.arsc`.
//!
//! The format is a chunk stream: a string pool, a map from the first
//! strings to attribute resource ids, then start and end element chunks.
//! Each attribute carries its value already typed (`Res_value`).

use anyhow::{Result, bail};

/// A `Res_value`.
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Null,
    /// A reference to a resource id, `@type/name`.
    Ref(u32),
    /// A reference to a theme attribute, `?attr/name`.
    Attr(u32),
    Str(String),
    Float(f32),
    /// A dimension: its value and unit (0 px, 1 dp, 2 sp, 3 pt, 4 in, 5 mm).
    Dim(f32, u8),
    /// A fraction: its value, and whether it is of the parent (`%p`).
    Frac(f32, bool),
    Int(i32),
    Bool(bool),
    /// A colour, ARGB.
    Color(u32),
}

fn u16_at(b: &[u8], i: usize) -> Result<u16> {
    match b.get(i..i + 2) {
        Some(s) => Ok(u16::from_le_bytes([s[0], s[1]])),
        None => bail!("truncated at {i}"),
    }
}

fn u32_at(b: &[u8], i: usize) -> Result<u32> {
    match b.get(i..i + 4) {
        Some(s) => Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]])),
        None => bail!("truncated at {i}"),
    }
}

/// A `ResStringPool` chunk at `at`.
pub fn string_pool(b: &[u8], at: usize) -> Result<Vec<String>> {
    if u16_at(b, at)? != 0x0001 {
        bail!("no string pool at {at}");
    }
    let header = usize::from(u16_at(b, at + 2)?);
    let count = u32_at(b, at + 8)? as usize;
    let utf8 = u32_at(b, at + 16)? & 0x100 != 0;
    let start = at + u32_at(b, at + 20)? as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let mut p = start + u32_at(b, at + header + 4 * i)? as usize;
        let s = if utf8 {
            // The length in UTF-16 units, then in bytes, each one or two
            // bytes long.
            let skip = |p: &mut usize| -> Result<usize> {
                let l = usize::from(*b.get(*p).ok_or_else(|| anyhow::anyhow!("truncated"))?);
                *p += 1;
                if l & 0x80 == 0 {
                    return Ok(l);
                }
                let l2 = usize::from(*b.get(*p).ok_or_else(|| anyhow::anyhow!("truncated"))?);
                *p += 1;
                Ok(((l & 0x7f) << 8) | l2)
            };
            skip(&mut p)?;
            let n = skip(&mut p)?;
            String::from_utf8_lossy(b.get(p..p + n).unwrap_or_default()).into_owned()
        } else {
            let mut n = usize::from(u16_at(b, p)?);
            p += 2;
            if n & 0x8000 != 0 {
                n = ((n & 0x7fff) << 16) | usize::from(u16_at(b, p)?);
                p += 2;
            }
            let units: Vec<u16> = (0..n)
                .map(|k| u16_at(b, p + 2 * k))
                .collect::<Result<_>>()?;
            String::from_utf16_lossy(&units)
        };
        out.push(s);
    }
    Ok(out)
}

/// A complex value's number: a 24-bit mantissa and a radix.
fn complex(data: u32) -> f32 {
    const RADIX: [f32; 4] = [
        1.0 / 256.0,
        1.0 / 32_768.0,
        1.0 / 8_388_608.0,
        1.0 / 2_147_483_648.0,
    ];
    ((data & 0xffff_ff00) as i32) as f32 * RADIX[((data >> 4) & 3) as usize]
}

/// A `Res_value` of type `ty` with `data`, strings from `pool`.
pub fn value(ty: u8, data: u32, pool: &[String]) -> Val {
    match ty {
        0x01 if data == 0 => Val::Null,
        0x01 => Val::Ref(data),
        0x02 => Val::Attr(data),
        0x03 => pool
            .get(data as usize)
            .map_or(Val::Null, |s| Val::Str(s.clone())),
        0x04 => Val::Float(f32::from_bits(data)),
        0x05 => Val::Dim(complex(data), (data & 0xf) as u8),
        0x06 => Val::Frac(complex(data), data & 0xf == 1),
        0x10 | 0x11 => Val::Int(data as i32),
        0x12 => Val::Bool(data != 0),
        0x1c | 0x1e => Val::Color(data),
        0x1d | 0x1f => Val::Color(data | 0xff00_0000),
        _ => Val::Null,
    }
}

#[derive(Clone, Debug)]
pub struct Attr {
    pub name: String,
    pub value: Val,
}

#[derive(Clone, Debug, Default)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<Attr>,
    pub children: Vec<Element>,
}

impl Element {
    pub fn get(&self, name: &str) -> Option<&Val> {
        self.attrs.iter().find(|a| a.name == name).map(|a| &a.value)
    }

    /// A string attribute.
    pub fn attr(&self, name: &str) -> Option<&str> {
        match self.get(name)? {
            Val::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// Parse a compiled XML document into its root element.
pub fn parse(b: &[u8]) -> Result<Element> {
    if u16_at(b, 0)? != 0x0003 {
        bail!("not compiled XML");
    }
    let mut p = usize::from(u16_at(b, 2)?);
    let mut pool = Vec::new();
    let mut ids: Vec<u32> = Vec::new();
    let mut stack: Vec<Element> = vec![Element::default()];
    while p + 8 <= b.len() {
        let ty = u16_at(b, p)?;
        let header = usize::from(u16_at(b, p + 2)?);
        let size = u32_at(b, p + 4)? as usize;
        if size < 8 {
            bail!("a chunk of {size} bytes at {p}");
        }
        match ty {
            0x0001 => pool = string_pool(b, p)?,
            0x0180 => {
                ids = (0..(size - header) / 4)
                    .map(|i| u32_at(b, p + header + 4 * i))
                    .collect::<Result<_>>()?;
            }
            0x0102 => {
                let ext = p + header;
                let name = pool
                    .get(u32_at(b, ext + 4)? as usize)
                    .cloned()
                    .unwrap_or_default();
                let start = usize::from(u16_at(b, ext + 8)?);
                let step = usize::from(u16_at(b, ext + 10)?);
                let count = usize::from(u16_at(b, ext + 12)?);
                let mut attrs = Vec::with_capacity(count);
                for i in 0..count {
                    let a = ext + start + i * step;
                    let ni = u32_at(b, a + 4)? as usize;
                    let mut aname = pool.get(ni).cloned().unwrap_or_default();
                    if aname.is_empty() {
                        // A shrunk APK can drop the name; the id stays.
                        aname = ids
                            .get(ni)
                            .map(|id| format!("0x{id:08x}"))
                            .unwrap_or_default();
                    }
                    let ty = *b.get(a + 15).ok_or_else(|| anyhow::anyhow!("truncated"))?;
                    let data = u32_at(b, a + 16)?;
                    attrs.push(Attr {
                        name: aname,
                        value: value(ty, data, &pool),
                    });
                }
                stack.push(Element {
                    name,
                    attrs,
                    children: vec![],
                });
            }
            0x0103 if stack.len() > 1 => {
                let done = stack.pop().unwrap_or_default();
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(done);
                }
            }
            _ => {}
        }
        p += size;
    }
    let mut doc = stack.into_iter().next().unwrap_or_default();
    match doc.children.len() {
        0 => bail!("no root element"),
        _ => Ok(doc.children.swap_remove(0)),
    }
}
