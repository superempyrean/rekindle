//! `resources.arsc`: the APK's resource table, and the lookup Android's
//! `Resources.getIdentifier` and `getDrawable` make with it.
//!
//! The Java platform resolves `res://<name>` as `q6.a` does: the drawable
//! named `<name>`, or, if the APK has none, the one
//! `assets/phoenix_fat_client_aliases.txt` maps it to (a line
//! `drawable <name> <alias>`). Of an entry's configurations, the one the
//! device would get is chosen: the night variant (the guest draws its dark
//! theme), then the density, then the newest SDK level.

use std::collections::HashMap;

use anyhow::{Result, bail};

use crate::axml::{Val, string_pool, value};

/// The qualifiers this lookup weighs. An entry with any other qualifier (a
/// locale, a layout direction, a screen size, a UI mode type) is never
/// chosen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    /// dpi; 0 is the default (160), 0xfffe `anydpi`, 0xffff `nodpi`.
    pub density: u16,
    pub night: Option<bool>,
    pub sdk: u16,
    /// Only the qualifiers above.
    pub plain: bool,
}

const ANY_DPI: u16 = 0xfffe;
/// The newest SDK level a resource qualifier may ask for.
const SDK: u16 = 36;

impl Config {
    fn parse(c: &[u8]) -> Config {
        let byte = |i: usize| c.get(i).copied().unwrap_or(0);
        let word = |i: usize| u16::from_le_bytes([byte(i), byte(i + 1)]);
        let ui = byte(29);
        let night = match ui & 0x30 {
            0x10 => Some(false),
            0x20 => Some(true),
            _ => None,
        };
        // Everything past the size word but density, sdk and the night bits.
        let plain =
            c.iter().enumerate().skip(4).all(|(i, &b)| {
                matches!(i, 14 | 15 | 24..=27) || (i == 29 && b & !0x30 == 0) || b == 0
            });
        Config {
            density: word(14),
            night,
            sdk: word(24),
            plain,
        }
    }
}

/// What a device asks of the table.
#[derive(Clone, Copy, Debug)]
pub struct Want {
    pub dpi: u16,
    pub night: bool,
}

impl Want {
    /// Android's preference, highest first: `anydpi`, the density asked
    /// for, the nearest density above it, then the nearest below.
    fn density_rank(self, d: u16) -> (u8, i32) {
        let d = if d == 0 { 160 } else { d };
        match d {
            ANY_DPI => (3, 0),
            d if d == self.dpi => (2, 0),
            d if d > self.dpi => (1, -i32::from(d)),
            d => (0, i32::from(d)),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Value {
    Val(Val),
    /// A style or other complex entry, which this lookup doesn't read.
    Bag,
}

#[derive(Default)]
pub struct Table {
    entries: HashMap<u32, Vec<(Config, Value)>>,
    names: HashMap<(String, String), u32>,
    ids: HashMap<u32, String>,
    /// `type/name` to the name the fat client gave it.
    aliases: HashMap<String, String>,
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

impl Table {
    /// The tables of the base and its splits, merged, and the fat client's
    /// aliases.
    pub fn parse(arscs: &[Vec<u8>], aliases: Option<&str>) -> Result<Table> {
        let mut t = Table::default();
        for b in arscs {
            t.add(b)?;
        }
        for line in aliases.unwrap_or_default().lines() {
            let mut w = line.split_whitespace();
            if let (Some(ty), Some(name), Some(alias)) = (w.next(), w.next(), w.next()) {
                t.aliases.insert(format!("{ty}/{name}"), alias.to_string());
            }
        }
        Ok(t)
    }

    fn add(&mut self, b: &[u8]) -> Result<()> {
        if u16_at(b, 0)? != 0x0002 {
            bail!("not a resource table");
        }
        let mut p = usize::from(u16_at(b, 2)?);
        let mut strings = Vec::new();
        while p + 8 <= b.len() {
            let ty = u16_at(b, p)?;
            let size = u32_at(b, p + 4)? as usize;
            match ty {
                0x0001 => strings = string_pool(b, p)?,
                0x0200 => self.package(&b[p..p + size], &strings)?,
                _ => {}
            }
            p += size.max(8);
        }
        Ok(())
    }

    fn package(&mut self, b: &[u8], strings: &[String]) -> Result<()> {
        let header = usize::from(u16_at(b, 2)?);
        let pkg = u32_at(b, 8)?;
        let types = string_pool(b, u32_at(b, 268)? as usize)?;
        let keys = string_pool(b, u32_at(b, 276)? as usize)?;
        let mut p = header;
        while p + 8 <= b.len() {
            let ty = u16_at(b, p)?;
            let chunk_header = usize::from(u16_at(b, p + 2)?);
            let size = u32_at(b, p + 4)? as usize;
            if ty == 0x0201 {
                let c = &b[p..p + size];
                self.type_chunk(c, chunk_header, pkg, &types, &keys, strings)?;
            }
            p += size.max(8);
        }
        Ok(())
    }

    fn type_chunk(
        &mut self,
        c: &[u8],
        header: usize,
        pkg: u32,
        types: &[String],
        keys: &[String],
        strings: &[String],
    ) -> Result<()> {
        let tid = u32::from(c[8]);
        let flags = c[9];
        let count = u32_at(c, 12)? as usize;
        let start = u32_at(c, 16)? as usize;
        let cfg = Config::parse(c.get(20..header).unwrap_or_default());
        let tname = types.get(tid as usize - 1).cloned().unwrap_or_default();
        // (index, offset into the entries).
        let slots: Vec<(usize, usize)> = if flags & 0x01 != 0 {
            (0..count)
                .map(|i| {
                    Ok((
                        usize::from(u16_at(c, header + 4 * i)?),
                        usize::from(u16_at(c, header + 4 * i + 2)?) * 4,
                    ))
                })
                .collect::<Result<_>>()?
        } else if flags & 0x02 != 0 {
            (0..count)
                .filter_map(|i| match u16_at(c, header + 2 * i) {
                    Ok(0xffff) => None,
                    Ok(o) => Some(Ok((i, usize::from(o) * 4))),
                    Err(e) => Some(Err(e)),
                })
                .collect::<Result<_>>()?
        } else {
            (0..count)
                .filter_map(|i| match u32_at(c, header + 4 * i) {
                    Ok(0xffff_ffff) => None,
                    Ok(o) => Some(Ok((i, o as usize))),
                    Err(e) => Some(Err(e)),
                })
                .collect::<Result<_>>()?
        };
        for (index, off) in slots {
            let e = start + off;
            let esize = u16_at(c, e)?;
            let eflags = u16_at(c, e + 2)?;
            let (key, v) = if eflags & 0x08 != 0 {
                // Compact: the key in the size field, the type in the high
                // byte of the flags.
                (
                    usize::from(esize),
                    Value::Val(value((eflags >> 8) as u8, u32_at(c, e + 4)?, strings)),
                )
            } else if eflags & 0x01 != 0 {
                (u32_at(c, e + 4)? as usize, Value::Bag)
            } else {
                let key = u32_at(c, e + 4)? as usize;
                (
                    key,
                    Value::Val(value(c[e + 11], u32_at(c, e + 12)?, strings)),
                )
            };
            let id = (pkg << 24) | (tid << 16) | index as u32;
            let kname = keys.get(key).cloned().unwrap_or_default();
            self.ids
                .entry(id)
                .or_insert_with(|| format!("{tname}/{kname}"));
            self.names.entry((tname.clone(), kname)).or_insert(id);
            self.entries.entry(id).or_default().push((cfg, v));
        }
        Ok(())
    }

    /// `getIdentifier(name, ty)`, then the fat client's alias for it.
    pub fn id(&self, ty: &str, name: &str) -> Option<u32> {
        let direct = |n: &str| self.names.get(&(ty.to_string(), n.to_string())).copied();
        direct(name).or_else(|| direct(self.aliases.get(&format!("{ty}/{name}"))?))
    }

    /// Every entry name of a type.
    #[cfg(test)]
    pub fn names_of(&self, ty: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .names
            .keys()
            .filter(|(t, _)| t == ty)
            .map(|(_, n)| n.clone())
            .collect();
        v.sort();
        v
    }

    /// The fat client's alias for `type/name`, if it has one.
    pub fn alias(&self, ty: &str, name: &str) -> Option<&str> {
        self.aliases
            .get(&format!("{ty}/{name}"))
            .map(String::as_str)
    }

    /// `type/name` of an id.
    pub fn name(&self, id: u32) -> String {
        self.ids
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("0x{id:08x}"))
    }

    /// The value the device would get for `id`.
    pub fn best(&self, id: u32, want: Want) -> Option<&Value> {
        self.entries
            .get(&id)?
            .iter()
            .filter(|(c, _)| c.plain && c.sdk <= SDK && c.night.is_none_or(|n| n == want.night))
            .max_by_key(|(c, _)| (c.night.is_some(), want.density_rank(c.density), c.sdk))
            .map(|(_, v)| v)
    }

    /// `v` with references followed to a plain value. A theme attribute
    /// stays as it is.
    pub fn resolve(&self, v: &Val, want: Want) -> Val {
        let mut v = v.clone();
        for _ in 0..16 {
            match v {
                Val::Ref(id) => match self.best(id, want) {
                    Some(Value::Val(next)) => v = next.clone(),
                    _ => return v,
                },
                _ => return v,
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn density_prefers_exact_then_nearest_above() {
        let w = Want {
            dpi: 200,
            night: true,
        };
        let mut ds = [160u16, 240, 320, 480, 0xfffe];
        ds.sort_by_key(|d| std::cmp::Reverse(w.density_rank(*d)));
        assert_eq!(ds, [0xfffe, 240, 320, 480, 160]);
    }
}
