## Project Context

deck-service is a Rust (Axum) HTTP service wrapping a C++ Project Sekai deck recommendation engine via FFI.

## Code Style

- Rust edition 2024, flat module structure (all `.rs` files in `src/`)
- Use `sonic_rs` for JSON (not `serde_json`)
- Wrap blocking C++ FFI calls with `tokio::task::block_in_place`
- All optional serde fields: `#[serde(skip_serializing_if = "Option::is_none")]`
- Minimal comments — only add when logic isn't self-evident

## Build

- C++ compiled via `zig c++` from `build.rs`
- Cross-compile: `cargo zigbuild --target x86_64-unknown-linux-musl`
- Docker: multi-stage build → `scratch` image (static musl binary)

## Architecture

```
handlers.rs → bridge.rs → ffi.rs → cpp_bridge/ → _cpp_src/ (C++ engine)
```

FFI boundary uses JSON strings. `DeckRecommend` handle is `Send` (not `Sync`), protected by `Mutex<DeckRecommend>` in `AppState`.

## Key Files

- `models.rs` — request/response types (mirrors upstream Python API)
- `cpp_bridge/deck_recommend_c.cpp` — C bridge using nlohmann/json
- `build.rs` — compiles C++ sources, creates static lib
