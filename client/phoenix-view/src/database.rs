//! `Database`, the guest's cache store, kept in memory.
//!
//! The guest opens a store by name with the tables it uses, and gets a handle
//! back. Each table maps a string key to a byte value. The Android module
//! keeps them in `cache.sqlite`, one per store. Here they live as long as the
//! run, so a run starts with an empty cache, as a fresh install does.
//!
//! A `Database.write` body is a sequence of ops, little-endian:
//!
//! ```text
//! u8 op           0 put, 1 delete, 2 delete every key with the prefix
//! u16 table len   table
//! u32 key len     key
//! u32 value len   value          (put only)
//! ```
//!
//! `getMany` and `scan` answer with their rows in the body, each a `u32`
//! key length, the key, a `u32` value length and the value, and the row
//! count in `count`.

use std::collections::{BTreeMap, HashMap};

use rekindle_wire::{PropId, Value};

/// Record keys, from the host's id table.
mod key {
    use rekindle_wire::PropId;
    pub const TABLES: PropId = PropId(398);
    pub const HANDLE: PropId = PropId(400);
    pub const CREATED: PropId = PropId(401);
    pub const ERROR: PropId = PropId(403);
    pub const TABLE: PropId = PropId(404);
    pub const KEY: PropId = PropId(405);
    pub const FOUND: PropId = PropId(406);
    pub const PREFIX: PropId = PropId(407);
    pub const KEYS_ONLY: PropId = PropId(408);
    pub const COUNT: PropId = PropId(409);
    pub const OK: PropId = PropId(410);
    pub const NAME: PropId = PropId(394);
    pub const AFTER: PropId = PropId(3137);
    pub const LIMIT: PropId = PropId(1632);
    pub const KEYS: PropId = PropId(2796);
}

/// The method ids.
pub mod method {
    pub const OPEN: u16 = 379;
    pub const GET: u16 = 380;
    pub const SCAN: u16 = 381;
    pub const WRITE: u16 = 382;
    pub const CLEAR: u16 = 383;
    pub const CLOSE: u16 = 387;
    pub const DESTROY: u16 = 388;
    pub const GET_MANY: u16 = 2838;
}

type Table = BTreeMap<String, Vec<u8>>;

/// A call's reply, record and body, if the module replies, and a note for
/// the log.
type Answer = (Option<(Value, Vec<u8>)>, String);

/// Every store the guest opened, by name.
#[derive(Default)]
pub struct Database {
    stores: HashMap<String, HashMap<String, Table>>,
    handles: HashMap<i32, String>,
    next: i32,
}

impl Database {
    /// Answer one `Database` call.
    pub fn call(&mut self, method: u16, record: &Value, body: &[u8]) -> Answer {
        match self.run(method, record, body) {
            Ok(answer) => answer,
            Err(e) => (
                Some((
                    Value::Object(vec![(key::ERROR, Value::Str(e.clone()))]),
                    vec![],
                )),
                format!("error: {e}"),
            ),
        }
    }

    fn run(&mut self, method: u16, record: &Value, body: &[u8]) -> Result<Answer, String> {
        let field = |k: PropId| match record {
            Value::Object(fields) => fields.iter().find(|(p, _)| *p == k).map(|(_, v)| v),
            _ => None,
        };
        let text = |k: PropId| match field(k) {
            Some(Value::Str(s)) => Ok(s.clone()),
            _ => Err(format!("missing field {}", k.0)),
        };
        let strings = |k: PropId| match field(k) {
            Some(Value::Array(items)) => Ok(items
                .iter()
                .filter_map(|v| match v {
                    Value::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()),
            _ => Err(format!("missing field {}", k.0)),
        };
        let handle = || {
            field(key::HANDLE)
                .and_then(Value::as_int)
                .and_then(|h| i32::try_from(h).ok())
                .ok_or_else(|| "missing field handle".to_string())
        };
        let ok = || Some((Value::Object(vec![(key::OK, Value::Bool(true))]), vec![]));
        match method {
            method::OPEN => {
                let name = text(key::NAME)?;
                let tables = strings(key::TABLES)?;
                let created = !self.stores.contains_key(&name);
                let store = self.stores.entry(name.clone()).or_default();
                for t in &tables {
                    store.entry(t.clone()).or_default();
                }
                self.next += 1;
                self.handles.insert(self.next, name.clone());
                Ok((
                    Some((
                        Value::Object(vec![
                            (key::HANDLE, Value::I32(self.next)),
                            (key::CREATED, Value::Bool(created)),
                        ]),
                        vec![],
                    )),
                    format!("{name:?}, {} tables -> handle {}", tables.len(), self.next),
                ))
            }
            method::CLOSE => {
                let h = handle()?;
                self.handles.remove(&h);
                Ok((ok(), format!("handle {h} closed")))
            }
            method::DESTROY => {
                let name = text(key::NAME)?;
                self.stores.remove(&name);
                Ok((ok(), format!("{name:?} destroyed")))
            }
            method::WRITE => {
                let store = self.store(handle()?)?;
                let ops = apply(store, body)?;
                Ok((ok(), format!("{ops} ops, {} bytes", body.len())))
            }
            method::CLEAR => {
                // With no table, every table.
                let table = text(key::TABLE).ok();
                for (name, t) in self.store(handle()?)? {
                    if table.as_ref().is_none_or(|w| w == name) {
                        t.clear();
                    }
                }
                Ok((
                    ok(),
                    format!("{} cleared", table.as_deref().unwrap_or("every table")),
                ))
            }
            method::GET => {
                let table = text(key::TABLE)?;
                let k = text(key::KEY)?;
                let value = self
                    .store(handle()?)?
                    .get(&table)
                    .and_then(|t| t.get(&k))
                    .cloned();
                let found = value.is_some();
                Ok((
                    Some((
                        Value::Object(vec![(key::FOUND, Value::Bool(found))]),
                        value.unwrap_or_default(),
                    )),
                    format!("{table}/{k}: found {found}"),
                ))
            }
            method::GET_MANY => {
                let table = text(key::TABLE)?;
                let keys = strings(key::KEYS)?;
                let store = self.store(handle()?)?;
                let t = store.get(&table);
                let mut out = Vec::new();
                let mut n = 0;
                for k in &keys {
                    if let Some(v) = t.and_then(|t| t.get(k)) {
                        row(&mut out, k.as_bytes(), v);
                        n += 1;
                    }
                }
                Ok((
                    Some((Value::Object(vec![(key::COUNT, Value::I32(n))]), out)),
                    format!("{table}: {n} of {} keys", keys.len()),
                ))
            }
            method::SCAN => {
                let table = text(key::TABLE)?;
                let prefix = text(key::PREFIX)?;
                let keys_only = field(key::KEYS_ONLY) == Some(&Value::Bool(true));
                let after = text(key::AFTER).ok();
                let limit = field(key::LIMIT)
                    .and_then(Value::as_int)
                    .map_or(usize::MAX, |l| l as usize);
                let store = self.store(handle()?)?;
                let mut out = Vec::new();
                let mut n = 0;
                if let Some(t) = store.get(&table) {
                    for (k, v) in t
                        .range(prefix.clone()..)
                        .take_while(|(k, _)| k.starts_with(&prefix))
                        .filter(|(k, _)| after.as_ref().is_none_or(|a| *k > a))
                        .take(limit)
                    {
                        row(&mut out, k.as_bytes(), if keys_only { &[] } else { v });
                        n += 1;
                    }
                }
                Ok((
                    Some((Value::Object(vec![(key::COUNT, Value::I32(n))]), out)),
                    format!("{table} {prefix:?}: {n} rows"),
                ))
            }
            _ => Ok((None, "no reply".into())),
        }
    }

    fn store(&mut self, handle: i32) -> Result<&mut HashMap<String, Table>, String> {
        let name = self.handles.get(&handle).ok_or("database not open")?;
        self.stores
            .get_mut(name)
            .ok_or_else(|| "database not open".into())
    }
}

fn row(out: &mut Vec<u8>, k: &[u8], v: &[u8]) {
    out.extend_from_slice(&(k.len() as u32).to_le_bytes());
    out.extend_from_slice(k);
    out.extend_from_slice(&(v.len() as u32).to_le_bytes());
    out.extend_from_slice(v);
}

/// Apply a write's ops to `store`, and return how many there were.
fn apply(store: &mut HashMap<String, Table>, mut b: &[u8]) -> Result<usize, String> {
    fn take<'a>(b: &mut &'a [u8], n: usize) -> Result<&'a [u8], String> {
        if b.len() < n {
            return Err("short write".into());
        }
        let (head, rest) = b.split_at(n);
        *b = rest;
        Ok(head)
    }
    fn len(b: &mut &[u8], width: usize) -> Result<usize, String> {
        let raw = take(b, width)?;
        Ok(raw
            .iter()
            .rev()
            .fold(0, |n, byte| (n << 8) | usize::from(*byte)))
    }
    let mut ops = 0;
    while !b.is_empty() {
        let op = take(&mut b, 1)?[0];
        let n = len(&mut b, 2)?;
        let table = String::from_utf8_lossy(take(&mut b, n)?).into_owned();
        let n = len(&mut b, 4)?;
        let key = String::from_utf8_lossy(take(&mut b, n)?).into_owned();
        let t = store.entry(table).or_default();
        match op {
            0 => {
                let n = len(&mut b, 4)?;
                t.insert(key, take(&mut b, n)?.to_vec());
            }
            1 => {
                t.remove(&key);
            }
            2 => t.retain(|k, _| !k.starts_with(&key)),
            other => return Err(format!("bad op {other}")),
        }
        ops += 1;
    }
    Ok(ops)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(table: &str, k: &str, v: &[u8]) -> Vec<u8> {
        let mut b = vec![0];
        b.extend_from_slice(&(table.len() as u16).to_le_bytes());
        b.extend_from_slice(table.as_bytes());
        b.extend_from_slice(&(k.len() as u32).to_le_bytes());
        b.extend_from_slice(k.as_bytes());
        b.extend_from_slice(&(v.len() as u32).to_le_bytes());
        b.extend_from_slice(v);
        b
    }

    fn rec(fields: Vec<(PropId, Value)>) -> Value {
        Value::Object(fields)
    }

    #[test]
    fn writes_then_reads() {
        let mut db = Database::default();
        let (open, _) = db.call(
            method::OPEN,
            &rec(vec![
                (key::NAME, Value::Str("main".into())),
                (key::TABLES, Value::Array(vec![Value::Str("users".into())])),
            ]),
            &[],
        );
        let Some((Value::Object(f), _)) = open else {
            panic!()
        };
        assert_eq!(f[0], (key::HANDLE, Value::I32(1)));
        let h = (key::HANDLE, Value::I32(1));
        let mut body = put("users", "1", b"one");
        body.extend(put("users", "2", b"two"));
        db.call(method::WRITE, &rec(vec![h.clone()]), &body);
        let (got, _) = db.call(
            method::GET,
            &rec(vec![
                h.clone(),
                (key::TABLE, Value::Str("users".into())),
                (key::KEY, Value::Str("2".into())),
            ]),
            &[],
        );
        assert_eq!(got.unwrap().1, b"two");
        let (scan, _) = db.call(
            method::SCAN,
            &rec(vec![
                h,
                (key::TABLE, Value::Str("users".into())),
                (key::PREFIX, Value::Str(String::new())),
                (key::KEYS_ONLY, Value::Bool(false)),
            ]),
            &[],
        );
        let (rec, body) = scan.unwrap();
        assert_eq!(rec, Value::Object(vec![(key::COUNT, Value::I32(2))]));
        assert_eq!(&body[..8], &[1, 0, 0, 0, b'1', 3, 0, 0]);
    }
}
