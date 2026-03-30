use std::env;
use std::path::Path;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let target = env::var("TARGET").unwrap();
    let root = Path::new(&manifest_dir);

    let cpp_src = root.join("_cpp_src/src");
    let json_include = root.join("_cpp_src/3rdparty/json/single_include");
    let bridge_dir = root.join("cpp_bridge");

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
        // musl cross-compile (via zig/cargo-zigbuild): static libc++
        println!("cargo:rustc-link-lib=static=c++");
        println!("cargo:rustc-link-lib=static=c++abi");
    } else if target.contains("linux") {
        // native Linux: libstdc++ (GCC)
        println!("cargo:rustc-link-lib=stdc++");
    } else {
        // macOS: libc++ (clang)
        println!("cargo:rustc-link-lib=c++");
    }

    println!("cargo:rerun-if-changed=cpp_bridge/");
    println!("cargo:rerun-if-changed=_cpp_src/src/");
}
