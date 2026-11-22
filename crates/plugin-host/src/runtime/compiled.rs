//! Compiled plugins, kept between launches.
//!
//! A plugin is compiled to machine code once per version and written to
//! `<dir>/<sha256>-<compat>.cwasm`, where `compat` is wasmtime's hash of
//! everything that makes compiled code usable (its version and settings,
//! the CPU's features). Later loads map that file instead of compiling.
//! The file is uncompressed, so loading it is a mapping rather than a read
//! and decompress, and the code pages are backed by the file instead of
//! the process's private memory.
//!
//! wasmtime runs what it finds there as machine code, without checking it,
//! so the directory must be as trusted as the host's own files.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use wasmtime::Engine;
use wasmtime::component::Component;

/// An artifact unused for this long is removed when another is written.
const KEEP_UNUSED: Duration = Duration::from_hours(1440);

#[derive(Clone, Debug)]
pub struct CompileCache {
    dir: PathBuf,
}

impl CompileCache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        CompileCache { dir: dir.into() }
    }

    fn path(&self, engine: &Engine, bytes: &[u8]) -> PathBuf {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(bytes);
        let sha = u128::from_be_bytes(digest[..16].try_into().expect("SHA-256 is 32 bytes"));
        let mut h = std::collections::hash_map::DefaultHasher::new();
        engine.precompile_compatibility_hash().hash(&mut h);
        self.dir
            .join(format!("{sha:032x}-{:016x}.cwasm", h.finish()))
    }

    /// `bytes` compiled: mapped from the cache, or compiled and kept. A
    /// cache that can't be read or written only costs the compile.
    pub fn component(&self, engine: &Engine, bytes: &[u8]) -> Result<Component> {
        let path = self.path(engine, bytes);
        if path.is_file() {
            // Last used now, for pruning.
            let _ = std::fs::File::options()
                .append(true)
                .open(&path)
                .and_then(|f| f.set_modified(SystemTime::now()));
            // SAFETY: the file was written by `precompile_component` below
            // into rekindle's own directory; wasmtime checks that it was
            // compiled by this version with these settings.
            match unsafe { Component::deserialize_file(engine, &path) } {
                Ok(c) => return Ok(c),
                Err(_) => {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        let compiled = engine.precompile_component(bytes)?;
        if self.store(&path, &compiled).is_ok() {
            // SAFETY: as above, just written.
            if let Ok(c) = unsafe { Component::deserialize_file(engine, &path) } {
                return Ok(c);
            }
        }
        // SAFETY: compiled by this engine a moment ago.
        Ok(unsafe { Component::deserialize(engine, &compiled) }?)
    }

    fn store(&self, path: &Path, compiled: &[u8]) -> std::io::Result<()> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        std::fs::create_dir_all(&self.dir)?;
        let tmp = path.with_extension(format!(
            "{}.{}.tmp",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&tmp, compiled)?;
        let renamed = std::fs::rename(&tmp, path);
        if renamed.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        self.prune();
        renamed
    }

    /// Remove artifacts unused for [`KEEP_UNUSED`]. One that another process
    /// has mapped stays until it can go.
    fn prune(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let now = SystemTime::now();
        for e in entries.flatten() {
            let p = e.path();
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| now.duration_since(t).is_ok_and(|age| age > KEEP_UNUSED));
            if p.extension().is_some_and(|x| x == "cwasm") && old {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}
