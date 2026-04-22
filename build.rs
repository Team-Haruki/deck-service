use std::env;
use std::path::{Path, PathBuf};

fn has_cpp_layout(path: &Path) -> bool {
    path.join("src").is_dir() && path.join("3rdparty").is_dir()
}

fn resolve_cpp_root(root: &Path) -> PathBuf {
    if let Ok(path) = env::var("DECK_CPP_SRC") {
        let candidate = PathBuf::from(path);
        if has_cpp_layout(&candidate) {
            return candidate;
        }
    }

    let bundled = root.join("_cpp_src");
    if has_cpp_layout(&bundled) {
        return bundled;
    }

    if let Some(parent) = root.parent() {
        let sibling = parent.join("sekai-deck-recommend-cpp");
        if has_cpp_layout(&sibling) {
            return sibling;
        }
    }

    panic!(
        "Unable to locate sekai-deck-recommend-cpp sources. Checked DECK_CPP_SRC, {} and sibling sekai-deck-recommend-cpp",
        bundled.display()
    );
}

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let host = env::var("HOST").unwrap();
    let target = env::var("TARGET").unwrap();
    let root = Path::new(&manifest_dir);
    let cpp_root = resolve_cpp_root(root);
    let cpp_src = cpp_root.join("src");
    let json_include = cpp_root.join("3rdparty/json/single_include");
    let bridge_dir = root.join("cpp_bridge");

    println!("cargo:warning=Using deck engine source at {}", cpp_root.display());
    println!("cargo:rerun-if-env-changed=DECK_CPP_SRC");

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .flag("-std=c++20")
        .opt_level_str("2")
        .warnings(false)
        .cpp_link_stdlib(None) // we handle stdlib linking ourselves
        .include(&cpp_src)
        .include(&json_include)
        .include(&bridge_dir);

    if host.contains("apple-darwin") && target.contains("musl") {
        build.archiver("/usr/bin/ar");
    }

    // C++ engine sources
    let sources = [
        "area-item-information/area-item-service.cpp",
        "card-information/card-calculator.cpp",
        "card-information/card-power-calculator.cpp",
        "card-information/card-service.cpp",
        "card-information/card-skill-calculator.cpp",
        "card-priority/card-priority-filter.cpp",
        "data-provider/data-provider.cpp",
        "data-provider/master-data.cpp",
        "data-provider/music-metas.cpp",
        "data-provider/static-data.cpp",
        "data-provider/user-data.cpp",
        "deck-information/deck-calculator.cpp",
        "deck-information/deck-service.cpp",
        "deck-recommend/base-deck-recommend.cpp",
        "deck-recommend/challenge-live-deck-recommend.cpp",
        "deck-recommend/deck-result-update.cpp",
        "deck-recommend/event-deck-recommend.cpp",
        "deck-recommend/find-best-cards-dfs.cpp",
        "deck-recommend/find-best-cards-ga.cpp",
        "deck-recommend/find-best-cards-sa.cpp",
        "deck-recommend/find-target-bonus-cards-dfs.cpp",
        "deck-recommend/find-worldbloom-target-bonus-cards-dfs.cpp",
        "deck-recommend/mysekai-deck-recommend.cpp",
        "event-point/card-bloom-event-calculator.cpp",
        "event-point/card-event-calculator.cpp",
        "event-point/event-calculator.cpp",
        "event-point/event-service.cpp",
        "live-score/live-calculator.cpp",
        "mysekai-information/mysekai-event-calculator.cpp",
        "mysekai-information/mysekai-service.cpp",
    ];

    for src in &sources {
        build.file(cpp_src.join(src));
    }

    // C bridge
    build.file(bridge_dir.join("deck_recommend_c.cpp"));

    build.compile("deck_recommend");

    // Link C++ standard library
    if target.contains("musl") {
        // Let zig resolve musl-target libc++/libc++abi during the final link.
        println!("cargo:rustc-link-lib=c++");
        println!("cargo:rustc-link-lib=c++abi");
    } else if target.contains("linux") {
        // native Linux: libstdc++ (GCC)
        println!("cargo:rustc-link-lib=stdc++");
    } else {
        // macOS: libc++ (clang)
        println!("cargo:rustc-link-lib=c++");
    }

    println!("cargo:rerun-if-changed=cpp_bridge/");
    println!("cargo:rerun-if-changed={}", cpp_src.display());
}
