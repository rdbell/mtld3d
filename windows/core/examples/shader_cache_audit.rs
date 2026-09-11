//! Replay a recorded cache through the library deduplicator without loading Wine.
//! Usage: `cargo run -p mtld3d-core --target aarch64-apple-darwin --example shader_cache_audit -- FILE WARM_COUNT`
//! Append `baseline SALT` or `dedup SALT` to time actual Metal compiles. Distinct alphanumeric
//! salts give each run fresh entry names, avoiding an already-warm driver source cache.
//! This measures library compilation, not application FPS or render-pipeline creation.

use std::{env, fs, time::Instant};

use mtld3d_core::shader_cache::{
    CacheEntry, CachedKind, CompiledShaders, SHADER_CACHE_SCHEMA_VERSION, read_header, read_records,
};
use objc2_foundation::NSString;
use objc2_metal::{
    MTLCompileOptions, MTLCreateSystemDefaultDevice, MTLDevice, MTLLanguageVersion, MTLLibrary,
    MTLMathMode,
};

fn main() {
    let args: Vec<_> = env::args().collect();
    assert!(
        matches!(args.len(), 3 | 5),
        "usage: shader_cache_audit FILE WARM_COUNT [baseline|dedup SALT]"
    );
    let warm: usize = args[2].parse().expect("WARM_COUNT must be an integer");
    let bytes = fs::read(&args[1]).expect("read shader cache");
    assert_eq!(read_header(&bytes), Ok(SHADER_CACHE_SCHEMA_VERSION));
    let (entries, _) = read_records(&bytes);
    assert!(warm <= entries.len());
    if args.len() == 5 {
        assert!(matches!(args[3].as_str(), "baseline" | "dedup"));
        assert!(!args[4].is_empty() && args[4].bytes().all(|c| c.is_ascii_alphanumeric()));
        metal_replay(&entries, warm, args[3] == "dedup", &args[4]);
        return;
    }
    let mut cache = CompiledShaders::default();
    let mut warm_unique = 0;
    for (index, entry) in entries.iter().enumerate() {
        let name = entry.kind.entry_name(entry.key);
        cache
            .resolve(entry.key, &entry.msl, &name, || Some(entry.key))
            .unwrap();
        if index + 1 == warm {
            warm_unique = cache.len();
        }
    }
    println!(
        "{{\"entries\":{},\"unique_sources\":{},\"duplicates\":{},\"warm_entries\":{warm},\"warm_unique\":{warm_unique},\"live_entries\":{},\"live_unique\":{}}}",
        entries.len(),
        cache.len(),
        cache.reused(),
        entries.len() - warm,
        cache.len() - warm_unique,
    );
}

fn metal_replay(entries: &[CacheEntry], warm: usize, dedup: bool, salt: &str) {
    let device = MTLCreateSystemDefaultDevice().expect("Metal device");
    let mut cache = CompiledShaders::default();
    let mut baseline_libraries = Vec::new();
    let mut calls = [0u32; 2];
    let mut elapsed_ms = [0.0f64; 2];
    let mut maximum_ms = [0.0f64; 2];
    let start = Instant::now();
    for (index, entry) in entries.iter().enumerate() {
        assert!(
            start.elapsed().as_secs() < 120,
            "Metal replay exceeded 120 seconds"
        );
        let name = entry.kind.entry_name(entry.key);
        let salted = format!("{name}_{salt}");
        let msl = entry.msl.replace(&name, &salted);
        let phase = usize::from(index >= warm);
        let mut compile = || {
            let started = Instant::now();
            let options = MTLCompileOptions::new();
            options.setLanguageVersion(MTLLanguageVersion::Version2_4);
            options.setPreserveInvariance(true);
            let vertex = matches!(
                entry.kind,
                CachedKind::FfVs | CachedKind::Sm1Vs | CachedKind::Sm2Vs | CachedKind::Sm3Vs
            );
            options.setMathMode(if vertex {
                MTLMathMode::Safe
            } else {
                MTLMathMode::Fast
            });
            let library = device
                .newLibraryWithSource_options_error(&NSString::from_str(&msl), Some(&options))
                .expect("recorded MSL must compile");
            let function = library
                .newFunctionWithName(&NSString::from_str(&salted))
                .expect("shader entry");
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            calls[phase] += 1;
            elapsed_ms[phase] += ms;
            maximum_ms[phase] = maximum_ms[phase].max(ms);
            Some((library, function))
        };
        if dedup {
            cache.resolve(entry.key, &msl, &salted, compile).unwrap();
        } else {
            baseline_libraries.push(compile().unwrap());
        }
    }
    // Keep baseline libraries alive through the replay, matching the encoder's ownership.
    assert!(dedup || baseline_libraries.len() == entries.len());
    println!(
        "{{\"dedup\":{dedup},\"warm_compiles\":{},\"live_compiles\":{},\"warm_compile_ms\":{:.3},\"live_compile_ms\":{:.3},\"max_live_compile_ms\":{:.3},\"wall_ms\":{:.3}}}",
        calls[0],
        calls[1],
        elapsed_ms[0],
        elapsed_ms[1],
        maximum_ms[1],
        start.elapsed().as_secs_f64() * 1000.0,
    );
}
