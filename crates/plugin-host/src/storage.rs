//! Per-plugin key/value storage with quotas.
//!
//! Each plugin gets its own namespace, either in memory (tests) or as one JSON
//! file per plugin under a directory. Writes go through to disk atomically.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct StorageQuota {
    pub max_keys: usize,
    pub max_key_bytes: usize,
    pub max_value_bytes: usize,
    pub max_total_bytes: usize,
}

impl Default for StorageQuota {
    fn default() -> Self {
        Self {
            max_keys: 1024,
            max_key_bytes: 256,
            max_value_bytes: 64 * 1024,
            max_total_bytes: 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StorageError {
    #[error("storage quota exceeded")]
    QuotaExceeded,
    #[error("key too long")]
    KeyTooLong,
    #[error("value too large")]
    ValueTooLarge,
}

#[derive(Debug)]
pub struct PluginStorage {
    data: BTreeMap<String, String>,
    path: Option<PathBuf>,
    quota: StorageQuota,
}

impl PluginStorage {
    pub fn in_memory(quota: StorageQuota) -> Self {
        Self {
            data: BTreeMap::new(),
            path: None,
            quota,
        }
    }

    /// Open (or lazily create) `dir/<plugin-id>.json`.
    pub fn open(dir: &std::path::Path, plugin_id: &str, quota: StorageQuota) -> Result<Self> {
        let path = dir.join(format!("{plugin_id}.json"));
        let data = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("corrupt storage file {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Ok(Self {
            data,
            path: Some(path),
            quota,
        })
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.data.get(key).map(String::as_str)
    }

    pub fn keys(&self) -> Vec<String> {
        self.data.keys().cloned().collect()
    }

    pub fn entries(&self) -> &BTreeMap<String, String> {
        &self.data
    }

    /// Bytes used (keys + values) and the quota.
    pub fn usage(&self) -> (u64, u64) {
        (self.total_bytes() as u64, self.quota.max_total_bytes as u64)
    }

    fn total_bytes(&self) -> usize {
        self.data.iter().map(|(k, v)| k.len() + v.len()).sum()
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<(), StorageError> {
        let q = self.quota;
        if key.len() > q.max_key_bytes {
            return Err(StorageError::KeyTooLong);
        }
        if value.len() > q.max_value_bytes {
            return Err(StorageError::ValueTooLarge);
        }
        let existing = self.data.get(key).map(|v| key.len() + v.len());
        if existing.is_none() && self.data.len() >= q.max_keys {
            return Err(StorageError::QuotaExceeded);
        }
        if self.total_bytes() - existing.unwrap_or(0) + key.len() + value.len() > q.max_total_bytes
        {
            return Err(StorageError::QuotaExceeded);
        }
        self.data.insert(key.to_string(), value.to_string());
        self.persist();
        Ok(())
    }

    pub fn delete(&mut self, key: &str) {
        if self.data.remove(key).is_some() {
            self.persist();
        }
    }

    fn persist(&self) {
        let Some(path) = &self.path else { return };
        let result = (|| -> Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(&self.data)?)?;
            std::fs::rename(&tmp, path)?;
            Ok(())
        })();
        if let Err(e) = result {
            tracing::warn!(path = %path.display(), "failed to persist plugin storage: {e:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotas() {
        let q = StorageQuota {
            max_keys: 2,
            max_key_bytes: 4,
            max_value_bytes: 8,
            max_total_bytes: 14,
        };
        let mut s = PluginStorage::in_memory(q);
        assert_eq!(s.set("toolong", "x"), Err(StorageError::KeyTooLong));
        assert_eq!(s.set("a", "123456789"), Err(StorageError::ValueTooLarge));
        s.set("a", "1234567").unwrap(); // 8 bytes
        assert_eq!(s.set("b", "123456"), Err(StorageError::QuotaExceeded)); // 15 > 14
        s.set("b", "12345").unwrap(); // 14
        assert_eq!(s.set("c", ""), Err(StorageError::QuotaExceeded)); // key count
        s.delete("b");
        s.set("", "").unwrap();
        s.set("", "").unwrap(); // an existing key, though it is empty
        s.set("b", "12345").unwrap_err();
        s.delete("");
        s.set("b", "12345").unwrap();
        s.set("a", "1").unwrap(); // overwrite shrinks
        s.delete("b");
        assert_eq!(s.keys(), vec!["a".to_string()]);
    }

    #[test]
    fn persists_to_disk() {
        let dir =
            std::env::temp_dir().join(format!("rekindle-storage-test-{}", std::process::id()));
        let mut s = PluginStorage::open(&dir, "dev.x.y", StorageQuota::default()).unwrap();
        s.set("k", "v").unwrap();
        let s2 = PluginStorage::open(&dir, "dev.x.y", StorageQuota::default()).unwrap();
        assert_eq!(s2.get("k"), Some("v"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
