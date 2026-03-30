use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Map a Cargo target triple to a zig target triple.
fn cargo_target_to_zig(target: &str) -> Option<&'static str> {
    match target {
        "x86_64-unknown-linux-musl" => Some("x86_64-linux-musl"),
        "x86_64-unknown-linux-gnu" => Some("x86_64-linux-gnu"),
        "aarch64-unknown-linux-musl" => Some("aarch64-linux-musl"),
        "aarch64-unknown-linux-gnu" => Some("aarch64-linux-gnu"),
        _ => None,
    }
}

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target_triple = env::var("TARGET").unwrap();
    let root = Path::new(&manifest_dir);

    let cpp_src = root.join("_cpp_src/src");
    let json_include = root.join("_cpp_src/3rdparty/json/single_include");
    let bridge_dir = root.join("cpp_bridge");

    let cpp_sources: Vec<PathBuf> = vec![
        cpp_src.join("area-item-information/area-item-service.cpp"),
        cpp_src.join("card-information/card-calculator.cpp"),
        cpp_src.join("card-information/card-power-calculator.cpp"),
        cpp_src.join("card-information/card-service.cpp"),
        cpp_src.join("card-information/card-skill-calculator.cpp"),
        cpp_src.join("card-priority/card-priority-filter.cpp"),
        cpp_src.join("data-provider/data-provider.cpp"),
        cpp_src.join("data-provider/master-data.cpp"),
        cpp_src.join("data-provider/music-metas.cpp"),
        cpp_src.join("data-provider/static-data.cpp"),
        cpp_src.join("data-provider/user-data.cpp"),
        cpp_src.join("deck-information/deck-calculator.cpp"),
        cpp_src.join("deck-information/deck-service.cpp"),
        cpp_src.join("deck-recommend/base-deck-recommend.cpp"),
        cpp_src.join("deck-recommend/challenge-live-deck-recommend.cpp"),
        cpp_src.join("deck-recommend/deck-result-update.cpp"),
        cpp_src.join("deck-recommend/event-deck-recommend.cpp"),
        cpp_src.join("deck-recommend/find-best-cards-dfs.cpp"),
        cpp_src.join("deck-recommend/find-best-cards-ga.cpp"),
        cpp_src.join("deck-recommend/find-best-cards-sa.cpp"),
        cpp_src.join("deck-recommend/find-target-bonus-cards-dfs.cpp"),
        cpp_src.join("deck-recommend/find-worldbloom-target-bonus-cards-dfs.cpp"),
        cpp_src.join("deck-recommend/mysekai-deck-recommend.cpp"),
        cpp_src.join("event-point/card-bloom-event-calculator.cpp"),
        cpp_src.join("event-point/card-event-calculator.cpp"),
        cpp_src.join("event-point/event-calculator.cpp"),
        cpp_src.join("event-point/event-service.cpp"),
        cpp_src.join("live-score/live-calculator.cpp"),
        cpp_src.join("mysekai-information/mysekai-event-calculator.cpp"),
        cpp_src.join("mysekai-information/mysekai-service.cpp"),
        // C bridge
        bridge_dir.join("deck_recommend_c.cpp"),
    ];

    let obj_dir = out_dir.join("cpp_obj");
    std::fs::create_dir_all(&obj_dir).unwrap();

    // Only use zig c++ when cross-compiling to a known zig target.
    // For native builds, use the host c++ to avoid libc++ vs libstdc++ mismatch on Linux.
    let zig_target = cargo_target_to_zig(&target_triple);
    let use_zig = zig_target.is_some();

    let mut objects = Vec::new();

    for src in &cpp_sources {
        let obj_name = src.file_stem().unwrap().to_str().unwrap().to_string();
        // Make unique names to avoid conflicts between files with the same base name
        let parent = src.parent().unwrap().file_name().unwrap().to_str().unwrap();
        let obj_file = obj_dir.join(format!("{parent}_{obj_name}.o"));

        let mut cmd = if use_zig {
            let mut c = Command::new("zig");
            c.arg("c++");
            c
        } else {
            Command::new("c++")
        };

        cmd.arg("-c")
            .arg("-std=c++20")
            .arg("-O2")
            .arg("-Wall")
            .arg(format!("-I{}", cpp_src.display()))
            .arg(format!("-I{}", json_include.display()))
            .arg(format!("-I{}", bridge_dir.display()));

        // Pass cross-compilation target if needed
        if let Some(zt) = zig_target {
            cmd.arg("-target").arg(zt);
        }

        cmd.arg(src).arg("-o").arg(&obj_file);

        let status = cmd.status().unwrap_or_else(|e| {
            panic!("Failed to compile {}: {}", src.display(), e);
        });
        if !status.success() {
            panic!("Compilation failed for {}", src.display());
        }

        objects.push(obj_file);
    }

    // Create static library: use zig ar when cross-compiling
    let lib_path = out_dir.join("libdeck_recommend.a");
    let mut ar_cmd = if zig_target.is_some() {
        let mut c = Command::new("zig");
        c.arg("ar");
        c
    } else {
        Command::new("ar")
    };
    ar_cmd.arg("rcs").arg(&lib_path);
    for obj in &objects {
        ar_cmd.arg(obj);
    }
    let status = ar_cmd.status().expect("Failed to run ar");
    if !status.success() {
        panic!("ar failed");
    }

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=deck_recommend");

    // Link C++ standard library
    if target_triple.contains("musl") {
        // musl cross-compile via zig: static libc++ + libc++abi
        println!("cargo:rustc-link-lib=static=c++");
        println!("cargo:rustc-link-lib=static=c++abi");
    } else if target_triple.contains("linux") {
        // native Linux (gnu): system has libstdc++ from GCC
        println!("cargo:rustc-link-lib=stdc++");
    } else {
        // macOS and others: clang's libc++
        println!("cargo:rustc-link-lib=c++");
    }

    // Re-run if C++ sources change
    println!("cargo:rerun-if-changed=cpp_bridge/");
    println!("cargo:rerun-if-changed=_cpp_src/src/");
}
