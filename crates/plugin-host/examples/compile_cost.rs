//! What a component's compile time depends on.
//!
//! `cargo run --release -p rekindle-plugin-host --example compile_cost -- <plugin.wasm>`
//!
//! Compiles one plugin under several engine settings, and its core modules
//! on their own, best of three each.

use std::time::{Duration, Instant};

fn best(f: impl Fn()) -> Duration {
    (0..3)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed()
        })
        .min()
        .unwrap()
}

fn config(edit: impl FnOnce(&mut wasmtime::Config)) -> wasmtime::Engine {
    let mut c = wasmtime::Config::new();
    c.wasm_component_model(true);
    c.consume_fuel(true);
    edit(&mut c);
    wasmtime::Engine::new(&c).unwrap()
}

/// The core modules inside a component (nested components included), as
/// (byte offset, bytes).
fn core_modules(b: &[u8]) -> Vec<&[u8]> {
    fn leb(b: &[u8], mut i: usize) -> (usize, usize) {
        let (mut r, mut s) = (0usize, 0);
        loop {
            let x = b[i];
            i += 1;
            r |= ((x & 0x7f) as usize) << s;
            s += 7;
            if x < 0x80 {
                return (r, i);
            }
        }
    }
    let mut out = Vec::new();
    let mut i = 8;
    while i < b.len() {
        let id = b[i];
        let (size, j) = leb(b, i + 1);
        match id {
            1 => out.push(&b[j..j + size]),
            4 => out.extend(core_modules(&b[j..j + size])),
            _ => {}
        }
        i = j + size;
    }
    out
}

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.len() > 1 {
        // Many plugins: one after another, then all at once on threads.
        let all: Vec<Vec<u8>> = paths.iter().map(|p| std::fs::read(p).unwrap()).collect();
        let e = config(|_| {});
        let t = Instant::now();
        for b in &all {
            drop(wasmtime::component::Component::new(&e, b).unwrap());
        }
        let serial = t.elapsed();
        let t = Instant::now();
        std::thread::scope(|s| {
            for b in &all {
                let e = &e;
                s.spawn(move || drop(wasmtime::component::Component::new(e, b).unwrap()));
            }
        });
        println!(
            "{} plugins: {:.0} ms one after another, {:.0} ms all at once",
            all.len(),
            serial.as_secs_f64() * 1000.0,
            t.elapsed().as_secs_f64() * 1000.0
        );
        return;
    }
    let path = paths.into_iter().next().expect("a plugin");
    let bytes = std::fs::read(&path).unwrap();
    let ms = |d: Duration| format!("{:>7.1} ms", d.as_secs_f64() * 1000.0);
    let compile = |e: &wasmtime::Engine| {
        best(|| drop(wasmtime::component::Component::new(e, &bytes).unwrap()))
    };

    println!("{path}");
    println!(
        "component, as the host compiles it      {}",
        ms(compile(&config(|_| {})))
    );
    println!(
        "  without fuel                          {}",
        ms(compile(&config(|c| {
            c.consume_fuel(false);
        })))
    );
    println!(
        "  without parallel compilation          {}",
        ms(compile(&config(|c| {
            c.parallel_compilation(false);
        })))
    );
    println!(
        "  Cranelift opt level none              {}",
        ms(compile(&config(|c| {
            c.cranelift_opt_level(wasmtime::OptLevel::None);
        })))
    );
    println!(
        "  single-pass register allocation       {}",
        ms(compile(&config(|c| {
            c.cranelift_regalloc_algorithm(wasmtime::RegallocAlgorithm::SinglePass);
        })))
    );
    let e = config(|_| {});
    let mut total = Duration::ZERO;
    for (n, m) in core_modules(&bytes).iter().enumerate() {
        let d = best(|| drop(wasmtime::Module::new(&e, m)));
        total += d;
        if d > Duration::from_millis(1) || m.len() > 10_000 {
            println!(
                "  core module {n}: {:>5} KB                {}",
                m.len() / 1024,
                ms(d)
            );
        }
    }
    println!("core modules on their own, in total     {}", ms(total));
}
