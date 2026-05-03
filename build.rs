use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const LIB_NAME: &str = "deck_recommend";
const BRIDGE_SOURCE: &str = "deck_recommend_c.cpp";

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

fn load_cpp_sources(root: &Path) -> Vec<String> {
    let source_list = root.join("cpp_sources.txt");
    let contents = fs::read_to_string(&source_list)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", source_list.display()));

    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

fn zig_target_for(rust_target: &str) -> &'static str {
    match rust_target {
        "aarch64-apple-darwin" => "aarch64-macos",
        "x86_64-apple-darwin" => "x86_64-macos",
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        "aarch64-unknown-linux-musl" => "aarch64-linux-musl",
        "x86_64-unknown-linux-musl" => "x86_64-linux-musl",
        other => panic!("unsupported Zig C++ target mapping for Rust target {other}"),
    }
}

fn use_libstdcpp(rust_target: &str) -> bool {
    rust_target.contains("linux") && rust_target.contains("gnu")
}

fn run_checked(command: &mut Command, description: &str) {
    let output = command
        .output()
        .unwrap_or_else(|err| panic!("failed to run {description}: {err}"));

    if output.status.success() {
        return;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    panic!(
        "{description} failed with status {}\n{stdout}{stderr}",
        output.status
    );
}

fn try_run(command: &mut Command) -> Result<(), String> {
    let output = command.output().map_err(|err| err.to_string())?;
    if output.status.success() {
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!("status {}\n{stdout}{stderr}", output.status))
}

fn static_lib_path(lib_dir: &Path) -> PathBuf {
    lib_dir.join(format!("lib{LIB_NAME}.a"))
}

fn run_zig_build(
    root: &Path,
    cpp_root: &Path,
    out_dir: &Path,
    zig_target: &str,
) -> Result<PathBuf, String> {
    let prefix = out_dir.join("zig-build");
    let mut command = Command::new("zig");
    command
        .current_dir(root)
        .arg("build")
        .arg(format!("-Dcpp-root={}", cpp_root.display()))
        .arg(format!("-Dtarget={zig_target}"))
        .arg("-Doptimize=ReleaseFast")
        .arg("--prefix")
        .arg(&prefix);

    try_run(&mut command)?;

    let lib_dir = prefix.join("lib");
    let lib_path = static_lib_path(&lib_dir);
    if !lib_path.is_file() {
        return Err(format!("zig build did not produce {}", lib_path.display()));
    }

    Ok(lib_dir)
}

fn object_name(index: usize, source: &str) -> String {
    let safe_source: String = source
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect();
    format!("{index:02}_{safe_source}.o")
}

fn compile_cpp_object(
    zig_target: &str,
    use_libstdcpp: bool,
    cpp_src: &Path,
    json_include: &Path,
    bridge_dir: &Path,
    source: &Path,
    object: &Path,
) {
    let mut command = Command::new("zig");
    command
        .arg("c++")
        .arg("-target")
        .arg(zig_target)
        .arg("-std=c++20")
        .arg("-O2")
        .arg("-fno-sanitize=all")
        .arg("-I")
        .arg(cpp_src)
        .arg("-I")
        .arg(json_include)
        .arg("-I")
        .arg(bridge_dir)
        .arg("-c")
        .arg(source)
        .arg("-o")
        .arg(object);

    if use_libstdcpp {
        command.arg("-stdlib=libstdc++");
    }

    run_checked(&mut command, &format!("compile {}", source.display()));
}

fn run_direct_zig_tools(
    root: &Path,
    cpp_root: &Path,
    out_dir: &Path,
    zig_target: &str,
    use_libstdcpp: bool,
) -> PathBuf {
    let cpp_src = cpp_root.join("src");
    let json_include = cpp_root.join("3rdparty/json/single_include");
    let bridge_dir = root.join("cpp_bridge");
    let sources = load_cpp_sources(root);
    let direct_dir = out_dir.join("zig-direct");
    let obj_dir = direct_dir.join("obj");
    let lib_dir = direct_dir.join("lib");

    let _ = fs::remove_dir_all(&obj_dir);
    fs::create_dir_all(&obj_dir)
        .unwrap_or_else(|err| panic!("failed to create {}: {err}", obj_dir.display()));
    fs::create_dir_all(&lib_dir)
        .unwrap_or_else(|err| panic!("failed to create {}: {err}", lib_dir.display()));

    let mut objects = Vec::with_capacity(sources.len() + 1);
    for (index, source) in sources.iter().enumerate() {
        let object = obj_dir.join(object_name(index, source));
        compile_cpp_object(
            zig_target,
            use_libstdcpp,
            &cpp_src,
            &json_include,
            &bridge_dir,
            &cpp_src.join(source),
            &object,
        );
        objects.push(object);
    }

    let bridge_object = obj_dir.join(object_name(sources.len(), BRIDGE_SOURCE));
    compile_cpp_object(
        zig_target,
        use_libstdcpp,
        &cpp_src,
        &json_include,
        &bridge_dir,
        &bridge_dir.join(BRIDGE_SOURCE),
        &bridge_object,
    );
    objects.push(bridge_object);

    let lib_path = static_lib_path(&lib_dir);
    let _ = fs::remove_file(&lib_path);

    let mut archive = Command::new("zig");
    archive.arg("ar").arg("cq").arg(&lib_path);
    archive.args(objects.iter().map(PathBuf::as_path));
    run_checked(&mut archive, "archive C++ objects");

    let mut index = Command::new("zig");
    index.arg("ar").arg("s").arg(&lib_path);
    run_checked(&mut index, "index C++ archive");

    lib_dir
}

fn emit_rerun_metadata(root: &Path, cpp_root: &Path) {
    println!("cargo:rerun-if-env-changed=DECK_CPP_SRC");
    println!(
        "cargo:rerun-if-changed={}",
        root.join("build.zig").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join("cpp_sources.txt").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join("cpp_bridge").display()
    );

    let cpp_src = cpp_root.join("src");
    for source in load_cpp_sources(root) {
        println!("cargo:rerun-if-changed={}", cpp_src.join(source).display());
    }
}

fn emit_cpp_stdlib_links(target: &str) {
    if use_libstdcpp(target) {
        println!("cargo:rustc-link-lib=stdc++");
    } else if target.contains("musl") {
        println!("cargo:rustc-link-lib=c++");
        println!("cargo:rustc-link-lib=c++abi");
    } else if target.contains("apple-darwin") {
        println!("cargo:rustc-link-lib=c++");
    }
}

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let host = env::var("HOST").unwrap();
    let target = env::var("TARGET").unwrap();
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let root = Path::new(&manifest_dir);
    let cpp_root = resolve_cpp_root(root);
    let zig_target = zig_target_for(&target);
    let use_libstdcpp = use_libstdcpp(&target);

    println!(
        "cargo:warning=Using deck engine source at {}",
        cpp_root.display()
    );
    emit_rerun_metadata(root, &cpp_root);

    let lib_dir = if host.contains("apple-darwin") {
        run_direct_zig_tools(root, &cpp_root, &out_dir, zig_target, use_libstdcpp)
    } else {
        match run_zig_build(root, &cpp_root, &out_dir, zig_target) {
            Ok(lib_dir) => lib_dir,
            Err(err) => panic!("zig build failed:\n{err}"),
        }
    };

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static={LIB_NAME}");
    emit_cpp_stdlib_links(&target);
}
