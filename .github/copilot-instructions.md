## Project Context

deck-service is a Rust (Axum) HTTP service wrapping Team Haruki's C++ Project Sekai deck recommendation engine via FFI. The current upstream source is `Team-Haruki/sekai-deck-recommend-cpp` on its default branch (`master`).

Upstream now ships Python bindings and a WebAssembly/npm package target. deck-service links the C++ core directly through `cpp_bridge/`; it does not import the upstream Python or npm packages.

## Code Style

- Rust edition 2024, flat module structure (all `.rs` files in `src/`)
- Use `sonic_rs` for JSON (not `serde_json`)
- Wrap blocking C++ FFI calls with `tokio::task::block_in_place`
- All optional serde fields: `#[serde(skip_serializing_if = "Option::is_none")]`
- Minimal comments — only add when logic isn't self-evident

## Build

- C++ compiled by `build.zig` for Zig-backed targets; native Linux GNU uses system `c++`/`ar`
- C++ source resolved from: `DECK_CPP_SRC` env → `_cpp_src/` → sibling `sekai-deck-recommend-cpp/`
- Clone source with submodules, e.g. `git clone --recursive https://github.com/Team-Haruki/sekai-deck-recommend-cpp.git _cpp_src`
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

All commit subjects must follow:

```
[Type] Short description starting with capital letter
```

Types: `[Feat]` new feature, `[Fix]` bug fix, `[Chore]` maintenance/refactor/build, `[Docs]` documentation.

Rules: capital letter start, imperative mood, no trailing period, <= ~70 chars.
Agent attribution uses a standard `Co-authored-by:` trailer in the commit body,
separated from the subject by a blank line.

Project examples:

```
[Feat] Add batch recommend endpoint with zstd framing
[Fix] Preload music metas on startup
[Chore] Update deck engine source ownership
[Docs] Document full obfuscated release builds
```

Agent-authored commit example:

```
[Docs] Add agent commit guidelines

Co-authored-by: Codex <noreply@openai.com>
```

## Key Files

- `models.rs` — request/response types (mirrors upstream Python API)
- `state.rs` — `AppState`, `EnginePool`, `UserdataCache`
- `masterdata.rs` — region-aware masterdata directory resolution
- `cpp_bridge/deck_recommend_c.cpp` — C bridge using nlohmann/json
- `build.zig` — compiles C++ sources and C bridge into the static archive for Zig-backed targets
- `build.rs` — Cargo glue for path resolution and link metadata
