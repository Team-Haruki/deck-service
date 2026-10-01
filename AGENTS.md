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

All data crosses the FFI boundary as JSON strings (via `sonic_rs::to_string` / `sonic_rs::from_str` on the Rust side, `yyjson` on the C++ side).

## Module Structure (flat)

All Rust source files are directly in `src/` — no nested modules:

| File | Responsibility |
| --- | --- |
| `main.rs` | Router setup, server entry point, env var handling, masterdata/musicmetas preloading, masterdata refresh watcher |
| `lib.rs` | Library facade re-exporting the modules below |
| `handlers.rs` | Axum route handler functions |
| `models.rs` | Serde request/response types (mirrors Python `.pyi` interface) |
| `bridge.rs` | Safe wrapper around FFI (owns the C++ handle, implements `Drop`) |
| `ffi.rs` | Raw `unsafe extern "C"` declarations + helper functions |
| `state.rs` | `AppState`, `EnginePool` (reader/writer concurrency), `UserdataCache` |
| `masterdata.rs` | Legacy masterdata directory resolution with region-aware candidate search (the directory path and `POST /update/masterdata` are deprecated) |
| `masterdata_audit.rs` | Master data key checks shared by the registry and JSON push paths: key normalisation, non-empty key tables, missing required/optional keys, and the 37-key lock tests |
| `registry.rs` | Master registry client: frozen 37-key engine list, manifest/blob/music-metas fetch, `ensure_region` (short-circuit on known `contentHash`, reload on change), preload + refresh loop, per-region `RegionMasterState` |
| `error.rs` | `AppError` enum with `IntoResponse` impl |

## Key Conventions

- **JSON library**: Use `sonic_rs`, never `serde_json`. Import `sonic_rs::json!` for constructing ad-hoc values.
- **Blocking FFI**: C++ calls are synchronous. Always wrap in `tokio::task::block_in_place` within async handlers.
- **Error handling**: Return `Result<_, AppError>` from handlers. `AppError::Engine(String)` for C++ errors, `AppError::BadRequest(String)` for input validation, `AppError::Timeout(String)` for pool timeouts.
- **FFI safety**: `DeckRecommend` is `Send` but not `Sync`. Concurrent access goes through `EnginePool`.
- **Optional fields**: All optional request fields use `#[serde(skip_serializing_if = "Option::is_none")]`.
- **Tests**: Unit tests live inline under `#[cfg(test)]` (native batch result merging in `handlers.rs`, env parsing helpers in `main.rs`); run with `cargo test`. The C++ engine itself is tested upstream.

## Concurrency Model

`EnginePool` in `state.rs` manages N `DeckRecommend` instances (default: `min(cpu_count, 4)`, configurable via `DECK_ENGINE_POOL_SIZE`). Uses `parking_lot::Mutex` + `Condvar` with two access patterns:

- **Reader** (`checkout`): acquires one engine slot for a single recommend call. Multiple readers run concurrently.
- **Writer** (`checkout_all`): acquires exclusive access to all engines for broadcast operations (masterdata/musicmeta updates). Blocks all readers; writer-priority prevents starvation.

Each engine slot tracks which userdata hashes it has loaded (`HashSet<String>`) to avoid redundant FFI calls. `UserdataCache` holds the actual userdata payloads server-side so any engine can replay them on demand.

`UserdataCache` (`src/userdata_cache.rs`) is an LRU bounded by `DECK_USERDATA_CACHE_MAX_ENTRIES` (128), `DECK_USERDATA_CACHE_MAX_BYTES` (256 MiB) and an idle `DECK_USERDATA_CACHE_TTL_SECONDS` (1800); each engine slot tracks at most 64 loaded hashes, the C++ `SharedUserdataStore` cap. `/cache_userdata` carries no region, so entries are tagged on use (`get(hash, Some(region))` from recommend, batch recommend and world bloom support cards). Every exclusive update (masterdata/musicmetas handlers, the directory refresh watcher, the registry path) must call `invalidate_userdata(..., UserdataInvalidation::Region(region))` rather than clearing the cache directly: it evicts entries tagged with that region plus untagged entries and prunes exactly those hashes from every engine slot. LRU and idle eviction inside the cache do not prune slot hash sets (no exclusive lease is held); a stale slot hash is harmless because the request fails at `resolve_userdata_payload` first. Lock order is pool, then cache; never take the cache lock and then the pool.

`DECK_ENGINE_THREADS` (default 1, clamped to available parallelism) sets the C++ engine's internal thread count. Keep `pool size × engine threads` within the CPU count; startup logs a warning when oversubscribed.

## Batch Recommendation (adaptive)

Batch `/recommend` picks its execution strategy from `DECK_ENGINE_THREADS`:

- **`= 1` (default)**: items fan out across the Rust `EnginePool` using scoped worker threads, one engine slot per worker.
- **`> 1`**: the whole batch is sent as one JSON array through a single native FFI call (`deck_recommend_recommend_batch_with_context_n`), letting the C++ engine parallelize internally; results are merged back per-item by `merge_native_batch_results` in `handlers.rs`.

## Binary Protocol

`/cache_userdata` and batch `/recommend` (content-type `application/octet-stream`) use zstd-compressed, length-prefixed segments: 4-byte big-endian length + payload per segment.

HTTP content coding is negotiated in `content_encoding.rs` (an Axum `from_fn` middleware outside `DefaultBodyLimit`): every response advertises `Accept-Encoding: zstd`; `Content-Encoding: zstd` request bodies are decoded with `ruzstd` under the same byte limit as identity bodies; JSON/text responses ≥ 1 KiB are zstd-encoded when the request accepts it. Keep it pure Rust (`ruzstd`), no C zstd dependency — the runtime image is `scratch`.

`GET /state/masterdata` reports `musicMetas` (region → sha256 of the loaded music metas); string pushes record their digest (`record_pushed_music_metas`) so Cloud can skip re-pushing identical metas after its own restart.

## Build System

- `build.zig` compiles the C++ source list from `cpp_sources.txt` + the C bridge into `libdeck_recommend.a` for Zig-backed targets
- `build.rs` resolves `DECK_CPP_SRC` / `_cpp_src` / sibling source paths, invokes Zig, and emits Cargo link metadata
- C++ source location resolved in order: `DECK_CPP_SRC` env → `_cpp_src/` → sibling `sekai-deck-recommend-cpp/`
- Fetch the pinned upstream source with `./scripts/prepare-cpp-engine.sh` (clones the commit in `cpp-engine.ref`, with submodules, into `_cpp_src/`)
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
- `/data` is read-only static engine data; `/cache` is owned by uid 65532 and holds the RL seed cache (`ENV DECK_RL_SEED_CACHE_FILE=/cache/rl_seed_cache.tsv`). No `VOLUME` instruction; deployments mount a named volume or a `chown 65532:65532` host dir at `/cache`
- `Dockerfile.runtime` uses a small alpine prep stage to create `/cache`, since `scratch` has no shell
- The startup log line `RL seed cache enabled` / not-writable warning / `disabled` is the deploy-time check

## Adding New Endpoints

1. Add request/response types to `models.rs`
2. Add handler function in `handlers.rs` (use `block_in_place` for FFI calls)
3. Register route in `main.rs`
4. If new C++ functionality is needed, extend `cpp_bridge/deck_recommend_c.h` and `.cpp`, then add the FFI declaration in `ffi.rs` and safe wrapper in `bridge.rs`

## Git commits

All commit subjects must follow:

```text
[Type] Short description starting with capital letter
```

Allowed types:

| Type      | Usage                                                 |
|-----------|-------------------------------------------------------|
| `[Feat]`  | New feature or capability                             |
| `[Fix]`   | Bug fix                                               |
| `[Chore]` | Maintenance, refactoring, dependency or build changes |
| `[Docs]`  | Documentation-only changes                            |

Rules:

- Description starts with a capital letter.
- Use imperative mood: `Add ...`, not `Added ...`.
- No trailing period.
- Keep the subject at or below roughly 70 characters.
- **Agent attribution uses the standard Git `Co-authored-by:` trailer in the commit body, not a free-form `Agent:` line.** This makes GitHub render the co-author avatar on the commit page. The trailer must be on its own line, separated from the subject by a blank line, in the form `Co-authored-by: <Display Name> <email>`. Suggested values per agent:
  - Claude (any model): `Co-authored-by: Claude Fable 5 <noreply@anthropic.com>` (substitute the actual model, e.g. `Claude Opus 4.7`, `Claude Sonnet 4.6`, `Claude Haiku 4.5`)
  - Codex: `Co-authored-by: Codex <noreply@openai.com>`
  - Copilot: `Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>`

Examples from this repo's history:

```text
[Feat] Optimize deck recommend bridge
[Fix] Include mutex for native bridge builds
[Chore] Update deck engine pin
[Chore] Align C++ engine source with master
```

## GitHub Actions workflows

CI reuses the shared templates in
[`seiunx-dev/ci-templates`](https://github.com/seiunx-dev/ci-templates) at `@v1`.
The files in `.github/workflows` are thin callers:

- The C++ engine commit lives only in `cpp-engine.ref`. `scripts/prepare-cpp-engine.sh`
  clones that commit (with submodules) into `_cpp_src/` for CI and release builds, and
  the Dockerfile COPYs and reads the same file. Bump the engine by editing
  `cpp-engine.ref` alone.
- `ci.yml` (`CI`) runs on `main` pushes, pull requests targeting `main`, and manual
  dispatch:
  - `Rust` (`rust-ci`, setup hook `prepare-cpp-engine.sh`): fmt, clippy
    `--all-targets -D warnings`, `cargo test`.
  - `C++ bridge tests + coverage` (custom job in the caller; no template covers gcov):
    `scripts/ci-cpp-coverage.sh` runs the `cpp_bridge` test and the C test harness with
    `--coverage` and writes `coverage/cpp.xml` (Sonar generic format via gcovr).
  - `Sonar` scans that coverage (skipped green on Dependabot/fork PRs); `Workflow lint`
    runs actionlint.
  - `Docker` does not wait for the tests. PRs build only; on `main` it pushes the
    immutable `ghcr.io/team-haruki/deck-service:sha-<full sha>` and `:sha-<7 chars>` as
    soon as the build finishes; the `Docker tags` job (`docker-retag.yml`, after
    `CI OK`) then moves `:main` to that digest without rebuilding, so `:main` only
    follows commits whose `CI OK` passed.
- The aggregate job **`CI OK`** is the only required status check.
- `release.yml` (`Release`): bump `version` in `Cargo.toml` in a PR → merge and wait for
  `CI OK` on `main` → push the tag `v<version>`. `release-gate` refuses a tag that
  differs from `Cargo.toml` and waits for `CI OK` on the tagged commit; then the
  binaries are built (tags only), the `main` image `:sha-<sha>` is promoted (re-tagged,
  not rebuilt) to `:<version>`, `:<major>.<minor>` and `:latest`, and the GitHub Release
  is published with `SHA256SUMS-<tag>.txt`. Manual dispatch is a dry run: it builds the
  binaries and publishes nothing.
- Release assets are a contract: `deck-service-linux-x64.tar.gz` and
  `deck-service-macos-arm64.tar.gz`, each with the bare `deck-service` binary at the
  archive root. MejiroRina/SekaiColo's Dockerfile downloads
  `deck-service-linux-x64.tar.gz` (`DECK_SERVICE_RELEASE_URL`), so keep the names, the
  flat layout and the glibc (`x86_64-unknown-linux-gnu`) linkage unless SekaiColo
  changes too.

Workflow maintenance rules:

- Use the shared templates first. Add custom jobs or steps only when a template
  genuinely cannot meet the project's needs, keep them in the thin caller files, and
  add a comment explaining why.
- Template bugs and missing features are fixed upstream in `seiunx-dev/ci-templates`
  (new `v1.x.y` tag), not worked around here.
- Keep top-level `permissions: contents: read`; grant `packages: write` / `contents: write`
  only on the job that needs it.
- Do not suppress `githubactions:S7637` (full-SHA pins) in `sonar-project.properties`: the
  template's `sonar.yml` already ignores it for the `@v1` references.
- Third-party actions in caller-side custom steps are pinned to a full commit SHA with a
  `# vX.Y.Z` comment; Dependabot (`github-actions`) updates them and the template refs.
