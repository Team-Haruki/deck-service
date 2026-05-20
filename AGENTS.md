# Agents Guide

This document describes conventions and context for AI coding agents working on this project.

## Project Overview

deck-service is a Rust HTTP service (Axum) that wraps Team Haruki's C++ deck recommendation engine for Project Sekai via FFI. The C++ source lives in `_cpp_src/` (gitignored, cloned separately).

The current upstream source is [Team-Haruki/sekai-deck-recommend-cpp](https://github.com/Team-Haruki/sekai-deck-recommend-cpp) on its default branch (`master`). Upstream now also ships Python bindings and a WebAssembly/npm target; deck-service consumes the same C++ core directly through `cpp_bridge/`, not through those packages.

## Language & Toolchain

- **Rust** (edition 2024) with Axum 0.8, Tokio, sonic-rs (not serde_json)
- **C++20** compiled by `build.zig` for Zig targets; native Linux GNU uses system `c++`/`ar`
- **Zig** is used only as a C++ compiler toolchain, not as the project language
- Cross-compilation: `cargo zigbuild --target x86_64-unknown-linux-musl`
- Upstream package tooling such as CMake, Python/uv, and emsdk is only needed when working in the C++ repository's Python or WebAssembly package targets.

## Architecture

```
Axum handlers → bridge.rs (safe Rust) → ffi.rs (unsafe extern "C") → C bridge (cpp_bridge/) → C++ engine (_cpp_src/)
```

All data crosses the FFI boundary as JSON strings (via `sonic_rs::to_string` / `sonic_rs::from_str` on the Rust side, `nlohmann::json` on the C++ side).

## Module Structure (flat)

All Rust source files are directly in `src/` — no nested modules:

| File | Responsibility |
| --- | --- |
| `main.rs` | Router setup, server entry point, env var handling, masterdata preloading |
| `handlers.rs` | Axum route handler functions |
| `models.rs` | Serde request/response types (mirrors Python `.pyi` interface) |
| `bridge.rs` | Safe wrapper around FFI (owns the C++ handle, implements `Drop`) |
| `ffi.rs` | Raw `unsafe extern "C"` declarations + helper functions |
| `state.rs` | `AppState`, `EnginePool` (reader/writer concurrency), `UserdataCache` |
| `masterdata.rs` | Masterdata directory resolution with region-aware candidate search |
| `error.rs` | `AppError` enum with `IntoResponse` impl |

## Key Conventions

- **JSON library**: Use `sonic_rs`, never `serde_json`. Import `sonic_rs::json!` for constructing ad-hoc values.
- **Blocking FFI**: C++ calls are synchronous. Always wrap in `tokio::task::block_in_place` within async handlers.
- **Error handling**: Return `Result<_, AppError>` from handlers. `AppError::Engine(String)` for C++ errors, `AppError::BadRequest(String)` for input validation, `AppError::Timeout(String)` for pool timeouts.
- **FFI safety**: `DeckRecommend` is `Send` but not `Sync`. Concurrent access goes through `EnginePool`.
- **Optional fields**: All optional request fields use `#[serde(skip_serializing_if = "Option::is_none")]`.
- **No tests yet**: The project currently has no Rust tests. The C++ engine is tested upstream.

## Concurrency Model

`EnginePool` in `state.rs` manages N `DeckRecommend` instances (default: `min(cpu_count, 4)`, configurable via `DECK_ENGINE_POOL_SIZE`). Uses `parking_lot::Mutex` + `Condvar` with two access patterns:

- **Reader** (`checkout`): acquires one engine slot for a single recommend call. Multiple readers run concurrently.
- **Writer** (`checkout_all`): acquires exclusive access to all engines for broadcast operations (masterdata/musicmeta updates). Blocks all readers; writer-priority prevents starvation.

Each engine slot tracks which userdata hashes it has loaded (`HashSet<String>`) to avoid redundant FFI calls. `UserdataCache` holds the actual userdata payloads server-side so any engine can replay them on demand.

## Binary Protocol

`/cache_userdata` and batch `/recommend` (content-type `application/octet-stream`) use zstd-compressed, length-prefixed segments: 4-byte big-endian length + payload per segment.

## Build System

- `build.zig` compiles the C++ source list from `cpp_sources.txt` + the C bridge into `libdeck_recommend.a` for Zig-backed targets
- `build.rs` resolves `DECK_CPP_SRC` / `_cpp_src` / sibling source paths, invokes Zig, and emits Cargo link metadata
- C++ source location resolved in order: `DECK_CPP_SRC` env → `_cpp_src/` → sibling `sekai-deck-recommend-cpp/`
- Clone the upstream source with submodules, for example `git clone --recursive https://github.com/Team-Haruki/sekai-deck-recommend-cpp.git _cpp_src`
- For musl targets, links `c++` and `c++abi` statically; macOS uses `c++`; Linux-gnu uses `stdc++`
- Native Linux-gnu host builds use system `c++`/`ar` to avoid mixing system libstdc++ headers with Zig glibc headers

## C++ Bridge (`cpp_bridge/`)

- `deck_recommend_c.h` — C API with opaque `DeckRecommendHandle`
- `deck_recommend_c.cpp` — Full implementation that parses JSON options and calls the C++ engine
- Error convention: functions return `const char*` (NULL = success, non-NULL = error message). Caller must free with `deck_recommend_free_string`.
- The `recommend` function returns a JSON result string and takes an `error_out` parameter.

## Docker

- Uses multi-stage build: zig+rust builder → `scratch` final image
- Output is a static musl binary with zero runtime dependencies
- No TLS/certificate libraries needed (service is behind a reverse proxy)

## Git Commit Format

All commit subjects must follow:

```
[Type] Short description starting with capital letter
```

| Type | Usage |
| --- | --- |
| `[Feat]` | New feature or capability |
| `[Fix]` | Bug fix |
| `[Chore]` | Maintenance, refactoring, dependency or build changes |
| `[Docs]` | Documentation-only changes |

Rules:
- Description starts with a capital letter.
- Use imperative mood: `Add ...`, not `Added ...`.
- No trailing period.
- Keep the subject at or below roughly 70 characters.
- Agent attribution uses the standard Git `Co-authored-by:` trailer in the
  commit body, not a free-form `Agent:` line.
- The trailer must be on its own line, separated from the subject by a blank
  line, in the form `Co-authored-by: <Display Name> <email>`.

Suggested values per agent:

- Claude (any 4.x): `Co-authored-by: Claude Opus 4.7 <noreply@anthropic.com>`
  (substitute the actual model, e.g. `Claude Sonnet 4.6`)
- Codex: `Co-authored-by: Codex <noreply@openai.com>`
- Copilot: `Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>`

Examples:

```
[Feat] Add batch recommend endpoint with zstd framing
```

```
[Fix] Preload music metas on startup
```

```
[Chore] Update deck engine source ownership
```

```
[Docs] Document full obfuscated release builds
```

Agent-authored commit example:

```
[Docs] Add agent commit guidelines

Co-authored-by: Codex <noreply@openai.com>
```

## Adding New Endpoints

1. Add request/response types to `models.rs`
2. Add handler function in `handlers.rs` (use `block_in_place` for FFI calls)
3. Register route in `main.rs`
4. If new C++ functionality is needed, extend `cpp_bridge/deck_recommend_c.h` and `.cpp`, then add the FFI declaration in `ffi.rs` and safe wrapper in `bridge.rs`
