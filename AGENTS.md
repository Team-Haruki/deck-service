# Agents Guide

This document describes conventions and context for AI coding agents working on this project.

## Project Overview

deck-service is a Rust HTTP service (Axum) that wraps Team Haruki's C++ deck recommendation engine for Project Sekai via FFI. The C++ source lives in `_cpp_src/` (gitignored, cloned separately).

The current upstream source is [Team-Haruki/sekai-deck-recommend-cpp](https://github.com/Team-Haruki/sekai-deck-recommend-cpp) on its default branch (`master`). Upstream now also ships Python bindings and a WebAssembly/npm target; deck-service consumes the same C++ core directly through `cpp_bridge/`, not through those packages.

## Language & Toolchain

- **Rust** (edition 2024) with Axum 0.8, Tokio, sonic-rs (not serde_json)
- **C++20**: native builds (host == target, Linux GNU or macOS) use the system `c++`/`ar`; cross builds use Zig (`build.zig` on non-macOS hosts, `zig c++` per object on macOS hosts)
- **Zig** (0.15.x; the Dockerfile pins 0.15.2, and `build.zig` uses the 0.15 `std.ArrayList` API) is used only as a C++ compiler toolchain, not as the project language
- Cross-compilation: `cargo zigbuild --target x86_64-unknown-linux-musl`
- Upstream package tooling such as CMake, Python/uv, and emsdk is only needed when working in the C++ repository's Python or WebAssembly package targets.

## Build & Run

**Prerequisites:** Rust >= 1.85 (edition 2024); for cross builds Zig 0.15.x and `cargo-zigbuild`; for native Linux GNU builds a system C++ compiler with libstdc++ headers

```bash
# Clone the pinned C++ engine (commit in cpp-engine.ref) into _cpp_src (gitignored, required for build)
./scripts/prepare-cpp-engine.sh

# Native build
cargo build --release

# Cross-compile to a static Linux binary (musl)
cargo zigbuild --release --target x86_64-unknown-linux-musl

# Run; DECK_DATA_DIR defaults to ../../_cpp_src/data relative to the executable's directory,
# which is this checkout's _cpp_src/data for target/<profile>/deck-service
DECK_DATA_DIR=./_cpp_src/data cargo run --release
```

`prepare-cpp-engine.sh` deletes and re-clones `_cpp_src/` whenever its `HEAD` is not the commit in `cpp-engine.ref`; keep local engine edits in a separate checkout and point `DECK_CPP_SRC` at it.

## Testing

- `cargo test` runs the inline `#[cfg(test)]` modules (`content_encoding.rs`, `handlers.rs`, `main.rs`, `masterdata_audit.rs`, `registry.rs`, `state.rs`, `userdata_cache.rs`) and the integration tests in `tests/`: `cpp_bridge.rs` (C bridge) and `jp_700_engine.rs` (JP 7.0.0 engine rules through the bridge with synthetic data; its real-data test runs only when `DECK_TEST_JP_MASTER_DIR` points at a JP 7.0.0+ master data directory, otherwise it skips).
- Tests load the engine's static data from the resolved C++ source (`DECK_CPP_SRC_DIR`, emitted by `build.rs`), so run `./scripts/prepare-cpp-engine.sh` first.
- `scripts/ci-cpp-coverage.sh [out.xml]` (needs `gcovr` on `PATH`) builds with `DECK_CPP_COVERAGE=1`, runs `cargo test --test cpp_bridge` and the C harness `tests/deck_recommend_c_test.cpp`, and writes a Sonar generic coverage report. `DECK_CPP_COVERAGE` only works for native Linux GNU builds.
- CI runs fmt, clippy and `cargo test` plus the coverage script; see [GitHub Actions workflows](#github-actions-workflows).

## Environment Variables

- `DECK_DATA_DIR` — C++ engine static data directory (read-only). Optional: unset falls back to `_cpp_src/data` relative to the executable (see Build & Run); the Docker image sets `/data`
- `DECK_RL_SEED_CACHE_FILE` / `DECK_RL_SEED_CACHE_DISABLE` — engine RL seed cache file (unset → `$DECK_DATA_DIR/rl_seed_cache.tsv`; the image sets `/cache/rl_seed_cache.tsv`) and the literal `1` kill switch; `main.rs` probes writability at startup and logs enabled / disabled / not-writable
- `DECK_REGISTRY_URL` — master registry base URL (plain `http`: `reqwest` is built without a TLS backend). When set, `DECK_REGISTRY_REGIONS` (default `jp,en,cn,tw,kr`) are pulled from the registry (`registry.rs`: manifest → the 38 engine keys by blob digest → `update_masterdata_from_json`, plus music metas) and the directory variables below only cover the remaining regions. `DECK_REGISTRY_REFRESH_MS` (300000, `0` disables), `DECK_REGISTRY_FETCH_CONCURRENCY` (8, clamped 1–64) and `DECK_REGISTRY_TIMEOUT_MS` (30000, at least 1000) tune it; `POST /update/masterdata/registry` and `GET /state/masterdata` expose it
- `DECK_MASTERDATA_DIR` / `DECK_MASTERDATA_BASE_DIR` — legacy (deprecated) masterdata directory preloaded on startup; `POST /update/masterdata` (`base_dir`) is deprecated in favour of `POST /update/masterdata/registry`
- `DECK_MASTERDATA_REGIONS` — legacy: CSV of regions to preload from the directory (default `jp,en,cn,tw,kr`)
- `DECK_MASTERDATA_REFRESH_MS` — legacy: directory refresh watcher interval (default 300000, `0` disables)
- `DECK_MUSICMETAS_DIR` / `DECK_MUSICMETAS_BASE_DIR` — music metas directory preloaded on startup (falls back to the masterdata directory, then `/app/data`)
- `DECK_MUSICMETAS_REGIONS` — CSV of music metas regions to preload (default `jp,en,cn,tw,kr`)
- `DECK_MUSICMETAS_FILE_<REGION>` — explicit music metas file for one region
- `DECK_ENGINE_POOL_SIZE` — number of engine instances (default `min(cpu_count, 4)`)
- `DECK_ENGINE_THREADS` — C++ engine-internal thread count (default 1)
- `DECK_USERDATA_CACHE_MAX_ENTRIES` / `DECK_USERDATA_CACHE_MAX_BYTES` / `DECK_USERDATA_CACHE_TTL_SECONDS` — userdata payload cache bounds (defaults 128 / 256 MiB / 1800 s)
- `DECK_RECOMMEND_TIMEOUT_MS` — default timeout injected when requests omit `timeout_ms`
- `DECK_LOCK_WARN_MS` / `DECK_LOCK_TIMEOUT_MS` / `DECK_ENGINE_WARN_MS` — pool wait warn threshold, pool acquire timeout, engine op warn threshold (defaults 1000 / 30000 / 10000 ms)
- `BIND_ADDR` — HTTP listen address (default `0.0.0.0:3000`)
- `RUST_LOG` — tracing `EnvFilter` directives
- Build time: `DECK_CPP_SRC` (C++ source override, see Build System), `DECK_CPP_COVERAGE` (gcov instrumentation, see Testing)

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
| `lib.rs` | Library crate (`deck_service`) declaring the modules below; `main.rs` and the integration tests use it |
| `handlers.rs` | Axum route handler functions |
| `models.rs` | Serde request/response types (mirrors Python `.pyi` interface) |
| `bridge.rs` | Safe wrapper around FFI (owns the C++ handle, implements `Drop`) |
| `ffi.rs` | Raw `unsafe extern "C"` declarations + helper functions |
| `state.rs` | `AppState`, `EnginePool` (reader/writer concurrency), `invalidate_userdata`; re-exports `UserdataCache` |
| `userdata_cache.rs` | `UserdataCache`: LRU userdata payload cache with byte budget, idle TTL and region tags; `CacheStats` for `GET /cache/stats` |
| `content_encoding.rs` | zstd HTTP content-coding middleware and the shared `MAX_BODY_BYTES` limit |
| `masterdata.rs` | Legacy masterdata directory resolution with region-aware candidate search (the directory path and `POST /update/masterdata` are deprecated) |
| `masterdata_audit.rs` | Master data key checks shared by the registry and JSON push paths: key normalisation, non-empty key tables, missing required/optional keys, and the lock tests for the 25 required + 13 optional engine keys |
| `registry.rs` | Master registry client: frozen engine key list (25 required + 13 optional = 38), manifest/blob/music-metas fetch, `ensure_region` (short-circuit on known `contentHash`, reload on change), preload + refresh loop, per-region `RegionMasterState` |
| `error.rs` | `AppError` enum with `IntoResponse` impl |

## Key Conventions

- **JSON library**: Use `sonic_rs`, never `serde_json`. Import `sonic_rs::json!` for constructing ad-hoc values.
- **Blocking FFI**: C++ calls are synchronous. Always wrap in `tokio::task::block_in_place` within async handlers.
- **Error handling**: Return `Result<_, AppError>` from handlers; errors render as `{"error": "..."}`. `Engine` (500) for C++ errors, `BadRequest` (400) for input validation, `UnprocessableEntity` (422) for deck constraints the engine cannot meet, `Timeout` (504) for pool timeouts, `UnsupportedMediaType` (415) for an unsupported `Content-Type` (the content-coding middleware returns its own 415 for unknown codings), `Upstream` (502) / `ServiceUnavailable` (503) for registry failures / registry not configured.
- **FFI safety**: `DeckRecommend` is `Send` but not `Sync`. Concurrent access goes through `EnginePool`.
- **Optional fields**: All optional request fields use `#[serde(skip_serializing_if = "Option::is_none")]`.
- **Comments**: Minimal — only where the logic is not self-evident.
- **Tests**: see [Testing](#testing).

## Concurrency Model

`EnginePool` in `state.rs` manages N `DeckRecommend` instances (default: `min(cpu_count, 4)`, configurable via `DECK_ENGINE_POOL_SIZE`). Uses `parking_lot::Mutex` + `Condvar` with two access patterns:

- **Reader** (`checkout`): acquires one engine slot for a single recommend call. Multiple readers run concurrently.
- **Writer** (`checkout_all`): acquires exclusive access to all engines for broadcast operations (masterdata/musicmeta updates). Blocks all readers; writer-priority prevents starvation.

Each engine slot tracks which userdata hashes it has loaded to avoid redundant FFI calls. `UserdataCache` holds the actual userdata payloads server-side so any engine can replay them on demand. Clients call `/cache_userdata` first and then reference the returned hash in `/recommend`, `/world_bloom/support_cards` and batch requests.

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
- Native builds (host == target) on Linux GNU and macOS use system `c++`/`ar` (`CXX`/`CC`/`AR` override them); on Linux GNU this avoids mixing system libstdc++ headers with Zig glibc headers
- Cross builds from a macOS host compile each object with `zig c++` directly; other cross builds go through `build.zig`

## C++ Bridge (`cpp_bridge/`)

- `deck_recommend_c.h` — C API with opaque `DeckRecommendHandle`
- `deck_recommend_c.cpp` — Full implementation that parses JSON options and calls the C++ engine
- `auto_score_policy.h` — finale AUTO score coefficient policy used by the bridge
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
