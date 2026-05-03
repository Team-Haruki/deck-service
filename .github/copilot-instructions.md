## Project Context

deck-service is a Rust (Axum) HTTP service wrapping a C++ Project Sekai deck recommendation engine via FFI.

## Code Style

- Rust edition 2024, flat module structure (all `.rs` files in `src/`)
- Use `sonic_rs` for JSON (not `serde_json`)
- Wrap blocking C++ FFI calls with `tokio::task::block_in_place`
- All optional serde fields: `#[serde(skip_serializing_if = "Option::is_none")]`
- Minimal comments — only add when logic isn't self-evident

## Build

- C++ compiled by `build.zig`; `build.rs` only resolves paths, invokes Zig, and emits Cargo link metadata
- C++ source resolved from: `DECK_CPP_SRC` env → `_cpp_src/` → sibling `sekai-deck-recommend-cpp/`
- Cross-compile: `cargo zigbuild --target x86_64-unknown-linux-musl`
- Docker: multi-stage build → `scratch` image (static musl binary)

## Architecture

```
handlers.rs → bridge.rs → ffi.rs → cpp_bridge/ → _cpp_src/ (C++ engine)
```

FFI boundary uses JSON strings. `DeckRecommend` handle is `Send` (not `Sync`), concurrent access goes through `EnginePool` in `state.rs` (reader/writer lock pattern with `parking_lot`).

## Concurrency

- `EnginePool` manages N engine instances (default: `min(cpu_count, 4)`)
- `checkout`: acquires one slot for recommend calls (concurrent readers)
- `checkout_all`: exclusive access for broadcast updates (masterdata/musicmetas)
- `UserdataCache` holds userdata payloads; each engine slot tracks loaded hashes to skip redundant FFI calls

## Git Commit Format

```
[Type] Short description starting with capital letter

Co-Authored-By: <agent name and email>
```

Types: `[Feat]` new feature, `[Fix]` bug fix, `[Chore]` maintenance/refactor/build, `[Docs]` documentation.

Rules: capital letter start, imperative mood, no trailing period, <= ~70 chars, always include `Co-Authored-By` trailer.

## Key Files

- `models.rs` — request/response types (mirrors upstream Python API)
- `state.rs` — `AppState`, `EnginePool`, `UserdataCache`
- `masterdata.rs` — region-aware masterdata directory resolution
- `cpp_bridge/deck_recommend_c.cpp` — C bridge using nlohmann/json
- `build.zig` — compiles C++ sources and C bridge into the static archive
- `build.rs` — Cargo glue for path resolution and link metadata
