//! How long plugins take to load, and where the time goes.
//!
//! `cargo run --release -p rekindle-plugin-host --example load_times -- <plugin.wasm>...`
//!
//! Loads every plugin into a runtime with an empty compile cache (a
//! plugin's first launch), then into a second runtime on the same cache
//! (every launch after), and prints each load's phases
//! ([`rekindle_plugin_host::runtime::LoadTimes`]) and the process's memory.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rekindle_plugin_host::runtime::{LoadTimes, StorageLocation};
use rekindle_plugin_host::{MockBackend, Runtime, RuntimeConfig, Trust};

fn ms(d: Duration) -> String {
    format!("{:>6.1}", d.as_secs_f64() * 1000.0)
}

/// The process's private bytes, in MB.
#[cfg(windows)]
fn private_mb() -> f64 {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        sizes: [usize; 8],
        private: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
    }
    let mut c = Counters {
        cb: std::mem::size_of::<Counters>() as u32,
        ..Default::default()
    };
    // SAFETY: `c` is a live, writable `Counters` whose `cb` holds its size,
    // which is the layout PROCESS_MEMORY_COUNTERS_EX has on x64, and the
    // pseudo-handle from GetCurrentProcess needs no closing.
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &raw mut c, c.cb) };
    c.private as f64 / 1e6
}
#[cfg(not(windows))]
fn private_mb() -> f64 {
    0.0
}

/// Load every plugin into a fresh runtime on `cache`; each load's phases and
/// the whole run's wall time.
fn run(
    plugins: &[(String, Vec<u8>)],
    cache: &std::path::Path,
    many: bool,
) -> (Vec<Option<LoadTimes>>, Duration, f64) {
    let before = private_mb();
    let backend = Arc::new(Mutex::new(MockBackend::new()));
    let config = RuntimeConfig {
        storage: StorageLocation::Memory,
        compile_cache: Some(cache.to_path_buf()),
        ..Default::default()
    };
    let t = Instant::now();
    let mut rt = Runtime::new(backend, config).expect("runtime");
    let mut times = Vec::new();
    if many {
        let list: Vec<(&[u8], Trust)> = plugins
            .iter()
            .map(|(_, b)| (b.as_slice(), Trust::FirstParty))
            .collect();
        for r in rt.load_many(&list) {
            if let Err(e) = r {
                eprintln!("{e:#}");
            }
        }
        let wall = t.elapsed();
        let grew = private_mb() - before;
        std::mem::forget(rt);
        return (Vec::new(), wall, grew);
    }
    for (name, bytes) in plugins {
        match rt.load(bytes, Trust::FirstParty) {
            Ok(_) => times.push(Some(rt.last_load)),
            Err(e) => {
                eprintln!("{name}: {e:#}");
                times.push(None);
            }
        }
    }
    let wall = t.elapsed();
    let grew = private_mb() - before;
    std::mem::forget(rt); // keep the plugins' memory counted until the end
    (times, wall, grew)
}

fn main() {
    let paths: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let plugins: Vec<(String, Vec<u8>)> = paths
        .iter()
        .map(|p| {
            (
                p.file_stem().unwrap().to_string_lossy().into_owned(),
                std::fs::read(p).expect("reading a plugin"),
            )
        })
        .collect();
    let cache = std::env::temp_dir().join(format!("rekindle-load-times-{}", std::process::id()));
    let (cold, cold_wall, cold_mem) = run(&plugins, &cache, false);
    let (warm, warm_wall, warm_mem) = run(&plugins, &cache, false);
    let fresh = cache.with_extension("many");
    let (_, many_cold, _) = run(&plugins, &fresh, true);
    let (_, many_warm, _) = run(&plugins, &fresh, true);
    let _ = std::fs::remove_dir_all(&fresh);

    println!(
        "{:<36} {:>6} | {:^27} | {:^27}",
        "", "", "first launch (cache cold)", "later launches (cache warm)"
    );
    println!(
        "{:<36} {:>6} | {:>6} {:>6} {:>6} {:>6} | {:>6} {:>6} {:>6} {:>6}",
        "plugin", "KB", "compile", "link", "inst", "act", "compile", "link", "inst", "act"
    );
    let row = |t: &Option<LoadTimes>| match t {
        Some(t) => format!(
            "{} {} {} {}",
            ms(t.compile),
            ms(t.link),
            ms(t.instantiate),
            ms(t.activate)
        ),
        None => format!("{:^27}", "failed"),
    };
    for (((name, bytes), c), w) in plugins.iter().zip(&cold).zip(&warm) {
        println!(
            "{name:<36} {:>6} | {} | {}",
            bytes.len() / 1024,
            row(c),
            row(w)
        );
    }
    let total = |ts: &[Option<LoadTimes>]| {
        let s = |f: fn(&LoadTimes) -> Duration| ts.iter().flatten().map(f).sum::<Duration>();
        format!(
            "{} {} {} {}",
            ms(s(|t| t.compile)),
            ms(s(|t| t.link)),
            ms(s(|t| t.instantiate)),
            ms(s(|t| t.activate))
        )
    };
    println!(
        "{:<36} {:>6} | {} | {}",
        "total",
        plugins.iter().map(|p| p.1.len()).sum::<usize>() / 1024,
        total(&cold),
        total(&warm)
    );
    println!(
        "wall: {} ms cold, {} ms warm (runtime included)",
        ms(cold_wall).trim(),
        ms(warm_wall).trim()
    );
    println!(
        "load_many: {} ms cold, {} ms warm",
        ms(many_cold).trim(),
        ms(many_warm).trim()
    );
    println!(
        "private memory added: {cold_mem:.0} MB cold (compiling included), {warm_mem:.0} MB warm"
    );
    let _ = std::fs::remove_dir_all(&cache);
}
