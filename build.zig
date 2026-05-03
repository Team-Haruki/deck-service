const std = @import("std");

fn cppRootPath(b: *std.Build, cpp_root: []const u8) std.Build.LazyPath {
    if (std.fs.path.isAbsolute(cpp_root)) {
        return .{ .cwd_relative = cpp_root };
    }
    return b.path(cpp_root);
}

fn loadCppSources(b: *std.Build) []const []const u8 {
    const source_list_path = b.pathFromRoot("cpp_sources.txt");
    const contents = std.fs.cwd().readFileAlloc(b.allocator, source_list_path, 64 * 1024) catch {
        @panic("failed to read cpp_sources.txt");
    };

    var sources: std.ArrayList([]const u8) = .empty;
    var lines = std.mem.tokenizeAny(u8, contents, "\r\n");
    while (lines.next()) |line| {
        const source = std.mem.trim(u8, line, " \t");
        if (source.len == 0 or source[0] == '#') {
            continue;
        }
        sources.append(b.allocator, b.dupe(source)) catch @panic("OOM");
    }

    return sources.toOwnedSlice(b.allocator) catch @panic("OOM");
}

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});
    const cpp_root = b.option([]const u8, "cpp-root", "C++ engine source root") orelse "_cpp_src";
    const libstdcpp_includes = b.option([]const []const u8, "libstdcpp-include", "System libstdc++ include path") orelse &.{};
    const root_path = cppRootPath(b, cpp_root);
    const use_libstdcpp = target.result.os.tag == .linux and target.result.abi == .gnu;

    if (use_libstdcpp and libstdcpp_includes.len == 0) {
        @panic("linux-gnu builds require at least one -Dlibstdcpp-include=<path>");
    }

    const root_module = b.createModule(.{
        .target = target,
        .optimize = optimize,
        .link_libc = true,
        .link_libcpp = !use_libstdcpp,
    });

    root_module.addIncludePath(root_path.path(b, "src"));
    root_module.addIncludePath(root_path.path(b, "3rdparty/json/single_include"));
    root_module.addIncludePath(b.path("cpp_bridge"));
    for (libstdcpp_includes) |include_path| {
        root_module.addSystemIncludePath(.{ .cwd_relative = include_path });
    }

    const cpp_flags = &[_][]const u8{
        "-std=c++20",
        "-O2",
        "-fno-sanitize=all",
    };

    root_module.addCSourceFiles(.{
        .root = root_path.path(b, "src"),
        .files = loadCppSources(b),
        .flags = cpp_flags,
    });

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
