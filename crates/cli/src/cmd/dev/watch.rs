//! A polling source watcher: compares mtimes and sizes, no OS notification API.
//!
//! Watches a plugin project's own files (`Cargo.toml` and `build.rs`, or
//! `package.json` and `tsconfig.json`) and everything
//! under `src/`. A change is reported once the tree has been quiet for
//! `settle`, so an editor's save-rename-write dance (or a formatter run)
//! causes one rebuild.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

type Snapshot = BTreeMap<PathBuf, (SystemTime, u64)>;

pub(super) struct Watcher {
    root: PathBuf,
    settle: Duration,
    snapshot: Snapshot,
    /// Paths changed since the last report, and when the most recent change was seen.
    pending: Vec<PathBuf>,
    last_change: Option<Instant>,
}

impl Watcher {
    pub(super) fn new(crate_dir: &Path, settle: Duration) -> Self {
        let root = crate_dir.to_path_buf();
        let snapshot = scan(&root);
        Self {
            root,
            settle,
            snapshot,
            pending: Vec::new(),
            last_change: None,
        }
    }

    /// Rescan. Returns the changed paths (relative to the crate) once the tree
    /// has been unchanged for `settle`; `None` while quiet or still settling.
    pub(super) fn poll(&mut self, now: Instant) -> Option<Vec<PathBuf>> {
        let next = scan(&self.root);
        let changed = diff(&self.snapshot, &next);
        self.snapshot = next;
        if !changed.is_empty() {
            for p in changed {
                let rel = p
                    .strip_prefix(&self.root)
                    .map(Path::to_path_buf)
                    .unwrap_or(p);
                if !self.pending.contains(&rel) {
                    self.pending.push(rel);
                }
            }
            self.last_change = Some(now);
            return None;
        }
        match self.last_change {
            Some(t) if now.saturating_duration_since(t) >= self.settle => {
                self.last_change = None;
                Some(std::mem::take(&mut self.pending))
            }
            _ => None,
        }
    }
}

fn scan(root: &Path) -> Snapshot {
    let mut out = Snapshot::new();
    for name in ["Cargo.toml", "build.rs", "package.json", "tsconfig.json"] {
        record(&root.join(name), &mut out);
    }
    walk(&root.join("src"), &mut out, 0);
    out
}

fn record(path: &Path, out: &mut Snapshot) {
    if let Ok(m) = std::fs::metadata(path)
        && m.is_file()
    {
        out.insert(
            path.to_path_buf(),
            (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len()),
        );
    }
}

/// Editor droppings that shouldn't trigger a rebuild.
fn ignored(name: &str) -> bool {
    let scratch = Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("swp") || e.eq_ignore_ascii_case("tmp"));
    name.starts_with('.') || name.ends_with('~') || scratch
}

fn walk(dir: &Path, out: &mut Snapshot, depth: usize) {
    if depth > 16 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        if ignored(&e.file_name().to_string_lossy()) {
            continue;
        }
        let path = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => walk(&path, out, depth + 1),
            Ok(_) => record(&path, out),
            Err(_) => {}
        }
    }
}

/// Paths added, removed or modified between two snapshots.
fn diff(old: &Snapshot, new: &Snapshot) -> Vec<PathBuf> {
    let mut changed: Vec<PathBuf> = new
        .iter()
        .filter(|(p, v)| old.get(*p) != Some(v))
        .map(|(p, _)| p.clone())
        .collect();
    changed.extend(old.keys().filter(|p| !new.contains_key(*p)).cloned());
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_crate(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rekindle-watch-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/nested")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]").unwrap();
        std::fs::write(dir.join("src/lib.rs"), "fn a() {}").unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        dir
    }

    #[test]
    fn reports_after_settling_and_only_once() {
        let dir = temp_crate("settle");
        let settle = Duration::from_millis(300);
        let mut w = Watcher::new(&dir, settle);
        let t0 = Instant::now();
        assert_eq!(w.poll(t0), None, "no changes yet");

        // Size changes are detected even if the mtime resolution is coarse.
        std::fs::write(dir.join("src/lib.rs"), "fn a() { /* edited */ }").unwrap();
        std::fs::write(dir.join("src/nested/new.rs"), "").unwrap();
        assert_eq!(w.poll(t0), None, "change seen, still settling");
        assert_eq!(w.poll(t0 + Duration::from_millis(100)), None);
        let mut got = w.poll(t0 + settle).expect("reported after settle");
        got.sort();
        assert_eq!(
            got,
            vec![
                PathBuf::from("src").join("lib.rs"),
                PathBuf::from("src").join("nested").join("new.rs")
            ]
        );
        assert_eq!(w.poll(t0 + settle * 3), None, "reported once");

        // Removal counts; files outside the watched set don't.
        std::fs::write(dir.join("target/out.wasm"), "x").unwrap();
        std::fs::write(dir.join("src/.lib.rs.swp"), "x").unwrap();
        std::fs::remove_file(dir.join("src/nested/new.rs")).unwrap();
        let t1 = t0 + settle * 4;
        assert_eq!(w.poll(t1), None);
        assert_eq!(
            w.poll(t1 + settle),
            Some(vec![PathBuf::from("src").join("nested").join("new.rs")])
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_change_restarts_the_settle_window() {
        let dir = temp_crate("restart");
        let settle = Duration::from_millis(300);
        let mut w = Watcher::new(&dir, settle);
        let t0 = Instant::now();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"").unwrap();
        assert_eq!(w.poll(t0), None);
        std::fs::write(dir.join("build.rs"), "fn main() {}").unwrap();
        assert_eq!(
            w.poll(t0 + Duration::from_millis(250)),
            None,
            "second change resets the window"
        );
        assert_eq!(w.poll(t0 + Duration::from_millis(400)), None);
        let got = w.poll(t0 + Duration::from_millis(560)).unwrap();
        assert_eq!(
            got,
            vec![PathBuf::from("Cargo.toml"), PathBuf::from("build.rs")]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn script_projects_watch_their_own_files() {
        let dir =
            std::env::temp_dir().join(format!("rekindle-watch-script-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        for f in ["package.json", "tsconfig.json", "src/index.ts"] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        let settle = Duration::from_millis(300);
        let mut w = Watcher::new(&dir, settle);
        let t0 = Instant::now();
        for f in ["package.json", "tsconfig.json", "src/index.ts"] {
            std::fs::write(dir.join(f), "edited").unwrap();
        }
        assert_eq!(w.poll(t0), None);
        let mut got = w.poll(t0 + settle).unwrap();
        got.sort();
        assert_eq!(
            got,
            vec![
                PathBuf::from("package.json"),
                PathBuf::from("src").join("index.ts"),
                PathBuf::from("tsconfig.json")
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
