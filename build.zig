const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const root_module = b.createModule(.{
        .target = target,
        .optimize = optimize,
        .link_libc = true,
        .link_libcpp = true,
    });

    // Include paths
    root_module.addIncludePath(b.path("_cpp_src/src"));
    root_module.addIncludePath(b.path("_cpp_src/3rdparty/json/single_include"));
    root_module.addIncludePath(b.path("cpp_bridge"));

    // All C++ source files from _cpp_src/src (excluding the pybind11 wrapper)
    const cpp_sources = &[_][]const u8{
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
    };

    const cpp_flags = &[_][]const u8{
        "-std=c++20",
        "-Wall",
        "-Wextra",
    };

    root_module.addCSourceFiles(.{
        .root = b.path("_cpp_src/src"),
        .files = cpp_sources,
        .flags = cpp_flags,
    });

    // C bridge source
    root_module.addCSourceFiles(.{
        .root = b.path("cpp_bridge"),
        .files = &.{"deck_recommend_c.cpp"},
        .flags = cpp_flags,
    });

    const lib = b.addLibrary(.{
        .linkage = .static,
        .name = "deck_recommend",
        .root_module = root_module,
    });

    b.installArtifact(lib);
}
