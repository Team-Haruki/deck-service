# Agents Guide

This document describes conventions and context for AI coding agents working on this project.

## Project Overview

deck-service is a Rust HTTP service (Axum) that wraps a C++ deck recommendation engine for Project Sekai via FFI. The C++ source lives in `_cpp_src/` (gitignored, cloned separately).

## Language & Toolchain

- **Rust** (edition 2024) with Axum 0.8, Tokio, sonic-rs (not serde_json)
- **C++20** compiled via `zig c++` (invoked from `build.rs`, not cmake/make)
- **Zig** is used only as a C++ compiler toolchain, not as the project language
- Cross-compilation: `cargo zigbuild --target x86_64-unknown-linux-musl`

## Architecture

```
Axum handlers → bridge.rs (safe Rust) → ffi.rs (unsafe extern "C") → C bridge (cpp_bridge/) → C++ engine (_cpp_src/)
```

All data crosses the FFI boundary as JSON strings (via `sonic_rs::to_string` / `sonic_rs::from_str` on the Rust side, `nlohmann::json` on the C++ side).

## Module Structure (flat)

All Rust source files are directly in `src/` — no nested modules:

| File | Responsibility |
| --- | --- |
| `main.rs` | Router setup, server entry point, env var handling |
| `handlers.rs` | Axum route handler functions |
| `models.rs` | Serde request/response types (mirrors Python `.pyi` interface) |
| `bridge.rs` | Safe wrapper around FFI (owns the C++ handle, implements `Drop`) |
| `ffi.rs` | Raw `unsafe extern "C"` declarations + helper functions |
| `state.rs` | `AppState` with `Mutex<DeckRecommend>` |
| `error.rs` | `AppError` enum with `IntoResponse` impl |

## Key Conventions

- **JSON library**: Use `sonic_rs`, never `serde_json`. Import `sonic_rs::json!` for constructing ad-hoc values.
- **Blocking FFI**: C++ calls are synchronous. Always wrap in `tokio::task::block_in_place` within async handlers.
- **Error handling**: Return `Result<_, AppError>` from handlers. `AppError::Engine(String)` for C++ errors, `AppError::BadRequest(String)` for input validation.
- **FFI safety**: `DeckRecommend` is `Send` but not `Sync`. Protect with `Mutex`. The handle is freed in `Drop`.
- **Optional fields**: All optional request fields use `#[serde(skip_serializing_if = "Option::is_none")]`.
- **No tests yet**: The project currently has no Rust tests. The C++ engine is tested upstream.

## Build System

- `build.rs` compiles 30+ C++ source files + the C bridge using `zig c++`
- Object files go to `OUT_DIR/cpp_obj/`, named `{parent_dir}_{filename}.o` to avoid collisions
- Static library `libdeck_recommend.a` is created with `ar` (or `zig ar` for cross-compile)
- For musl targets, link `c++` and `c++abi` statically

## C++ Bridge (`cpp_bridge/`)

- `deck_recommend_c.h` — C API with opaque `DeckRecommendHandle`
- `deck_recommend_c.cpp` — Full implementation that parses JSON options and calls the C++ engine
- Error convention: functions return `const char*` (NULL = success, non-NULL = error message). Caller must free with `deck_recommend_free_string`.
- The `recommend` function returns a JSON result string and takes an `error_out` parameter.

## Docker

- Uses multi-stage build: zig+rust builder → `scratch` final image
- Output is a static musl binary with zero runtime dependencies
- No TLS/certificate libraries needed (service is behind a reverse proxy)

## Adding New Endpoints

1. Add request/response types to `models.rs`
2. Add handler function in `handlers.rs` (use `block_in_place` for FFI calls)
3. Register route in `main.rs`
4. If new C++ functionality is needed, extend `cpp_bridge/deck_recommend_c.h` and `.cpp`, then add the FFI declaration in `ffi.rs` and safe wrapper in `bridge.rs`
