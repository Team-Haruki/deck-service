# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This Is

A Rust/Axum HTTP service wrapping a C++ Project Sekai deck recommendation engine via FFI. Requests flow through: `handlers.rs -> bridge.rs -> ffi.rs -> cpp_bridge/ -> _cpp_src/ (C++ engine)`. The FFI boundary uses JSON strings serialized with `sonic_rs`.

## Build & Run

**Prerequisites:** Rust >= 1.85, Zig >= 0.14, cargo-zigbuild

```bash
# Clone C++ engine source (gitignored, required for build)
git clone https://github.com/moe-sekai/sekai-deck-recommend-cpp _cpp_src
cd _cpp_src && git submodule update --init --recursive && cd ..

# Native build
cargo build --release

# Cross-compile to static Linux binary (musl)
cargo zigbuild --release --target x86_64-unknown-linux-musl

# Run (DECK_DATA_DIR is required)
DECK_DATA_DIR=./_cpp_src/data cargo run --release
```

The C++ static library is built by `build.zig`. Cargo uses `build.rs` only to resolve the C++ source location (`DECK_CPP_SRC` env, then `_cpp_src/`, then sibling `sekai-deck-recommend-cpp/`), invoke Zig, and emit link metadata. Linux GNU host builds use system libstdc++ headers discovered from `c++ -v`.

## Code Conventions

- Rust edition 2024, flat module layout (all `.rs` in `src/`)
- Use `sonic_rs` for all JSON serialization, not `serde_json`
- All optional serde fields use `#[serde(skip_serializing_if = "Option::is_none")]`
- Blocking C++ FFI calls must be wrapped with `tokio::task::block_in_place`
- `DeckRecommend` is `Send` but not `Sync` -- concurrent access goes through `EnginePool` (reader/writer lock pattern with `parking_lot`)
- Minimal comments -- only where logic isn't self-evident

## Concurrency Model

`EnginePool` in `state.rs` manages N `DeckRecommend` instances (default: `min(cpu_count, 4)`). Two access patterns:
- **Reader** (`checkout`): acquires one engine slot for a single recommend call. Multiple readers run concurrently.
- **Writer** (`checkout_all`): acquires exclusive access to all engines for broadcast operations (masterdata/musicmeta updates). Blocks all readers.

Userdata is cached server-side: clients call `/cache_userdata` first, then reference the returned hash in subsequent `/recommend` calls. Each engine slot tracks which userdata hashes it has loaded to avoid redundant FFI calls.

## Key Environment Variables

- `DECK_DATA_DIR` -- path to C++ engine static data (required at runtime)
- `DECK_MASTERDATA_DIR` / `DECK_MASTERDATA_BASE_DIR` -- masterdata directory for preloading on startup
- `DECK_MASTERDATA_REGIONS` -- CSV of regions to preload (default: jp,en,cn,tw,kr)
- `DECK_ENGINE_POOL_SIZE` -- number of engine instances
- `DECK_RECOMMEND_TIMEOUT_MS` -- default timeout injected when requests omit `timeout_ms`

## Binary Protocol

The `/cache_userdata` and batch `/recommend` endpoints accept `application/octet-stream` bodies: zstd-compressed, length-prefixed segments (4-byte big-endian length + payload per segment).

## Git Commit Format

All commits must follow:

```
[Type] Short description starting with capital letter

Co-Authored-By: <agent name and email>
```

| Type | Usage |
| --- | --- |
| `[Feat]` | New feature or capability |
| `[Fix]` | Bug fix |
| `[Chore]` | Maintenance, refactoring, dependency or build changes |
| `[Docs]` | Documentation-only changes |

Rules:
- Description starts with a capital letter
- Imperative mood (`Add ...`, not `Added ...`)
- No trailing period
- Keep subject <= ~70 chars
- Always include a `Co-Authored-By` trailer identifying the AI agent

Examples:

```
[Feat] Add batch recommend endpoint with zstd framing

Co-Authored-By: Claude <noreply@anthropic.com>
```

```
[Fix] Preload deck masterdata on startup

Co-Authored-By: Claude <noreply@anthropic.com>
```

```
[Chore] Migrate build system to cc crate

Co-Authored-By: Claude <noreply@anthropic.com>
```

```
[Docs] Update AGENTS.md with concurrency model

Co-Authored-By: Claude <noreply@anthropic.com>
```
