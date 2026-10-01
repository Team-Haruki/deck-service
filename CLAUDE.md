# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This Is

A Rust/Axum HTTP service wrapping Team Haruki's maintained C++ Project Sekai deck recommendation engine via FFI. Requests flow through: `handlers.rs -> bridge.rs -> ffi.rs -> cpp_bridge/ -> _cpp_src/ (C++ engine)`. The FFI boundary uses JSON strings serialized with `sonic_rs`.

The current upstream source is `Team-Haruki/sekai-deck-recommend-cpp` on its
default branch (`master`). Upstream now ships Python and WebAssembly/npm
package targets; deck-service links only the C++ core sources through its own
C bridge.

## Build & Run

**Prerequisites:** Rust >= 1.85, Zig >= 0.14, cargo-zigbuild

```bash
# Clone the pinned C++ engine (commit in cpp-engine.ref) into _cpp_src (gitignored, required for build)
./scripts/prepare-cpp-engine.sh

# Native build
cargo build --release

# Cross-compile to static Linux binary (musl)
cargo zigbuild --release --target x86_64-unknown-linux-musl

# Run (DECK_DATA_DIR is required)
DECK_DATA_DIR=./_cpp_src/data cargo run --release
```

The C++ static library is built by `build.zig` for Zig-backed targets. Cargo uses `build.rs` to resolve the C++ source location (`DECK_CPP_SRC` env, then `_cpp_src/`, then sibling `sekai-deck-recommend-cpp/`), invoke the right archive builder, and emit link metadata. Native Linux GNU host builds use system `c++`/`ar` to avoid mixing system libstdc++ headers with Zig glibc headers.

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

Userdata is cached server-side: clients call `/cache_userdata` first, then reference the returned hash in subsequent `/recommend` calls. Each engine slot tracks which userdata hashes it has loaded to avoid redundant FFI calls. `UserdataCache` (`src/userdata_cache.rs`) is an LRU bounded by entries, bytes and idle TTL whose entries are tagged with the regions that used them; exclusive region updates go through `invalidate_userdata` (`UserdataInvalidation::Region`), which drops that region's and never-used entries and prunes only those hashes from the engine slots.

## Key Environment Variables

- `DECK_DATA_DIR` -- path to C++ engine static data (required at runtime; read-only)
- `DECK_RL_SEED_CACHE_FILE` / `DECK_RL_SEED_CACHE_DISABLE` -- engine RL seed cache file (unset -> `$DECK_DATA_DIR/rl_seed_cache.tsv`; image sets `/cache/rl_seed_cache.tsv`) and literal `1` kill switch; `main.rs` probes writability at startup and logs enabled/disabled/not-writable
- `DECK_REGISTRY_URL` -- master registry base URL; when set, `DECK_REGISTRY_REGIONS` (default all five) are pulled from the registry (`registry.rs`: manifest → 37 engine keys by blob digest → `update_masterdata_from_json`, plus music metas) and the directory variables below only cover the remaining regions. `DECK_REGISTRY_REFRESH_MS` / `_FETCH_CONCURRENCY` / `_TIMEOUT_MS` tune it. `POST /update/masterdata/registry` and `GET /state/masterdata` expose it
- `DECK_MASTERDATA_DIR` / `DECK_MASTERDATA_BASE_DIR` -- legacy (deprecated) masterdata directory for preloading on startup; `POST /update/masterdata` (base_dir) is deprecated in favour of `POST /update/masterdata/registry`
- `DECK_MASTERDATA_REGIONS` -- legacy: CSV of regions to preload from the directory (default: jp,en,cn,tw,kr)
- `DECK_MUSICMETAS_DIR` / `DECK_MUSICMETAS_BASE_DIR` -- music metas directory for preloading on startup
- `DECK_MUSICMETAS_REGIONS` -- CSV of music metas regions to preload (default: jp,en,cn,tw,kr)
- `DECK_MUSICMETAS_FILE_<REGION>` -- explicit music metas file for one region
- `DECK_MASTERDATA_REFRESH_MS` -- legacy: masterdata directory refresh watcher interval (default: 300000)
- `DECK_ENGINE_POOL_SIZE` -- number of engine instances
- `DECK_USERDATA_CACHE_MAX_ENTRIES` / `DECK_USERDATA_CACHE_MAX_BYTES` / `DECK_USERDATA_CACHE_TTL_SECONDS` -- userdata payload cache bounds (defaults: 128 / 256 MiB / 1800 s)
- `DECK_ENGINE_THREADS` -- C++ engine-internal thread count (default: 1); keep `pool size x engine threads` within the CPU count
- `DECK_RECOMMEND_TIMEOUT_MS` -- default timeout injected when requests omit `timeout_ms`
- `DECK_LOCK_WARN_MS` / `DECK_LOCK_TIMEOUT_MS` / `DECK_ENGINE_WARN_MS` -- pool wait warn threshold, pool acquire timeout, engine op warn threshold
- `BIND_ADDR` -- HTTP listen address (default: 0.0.0.0:3000)

## Binary Protocol

The `/cache_userdata` and batch `/recommend` endpoints accept `application/octet-stream` bodies: zstd-compressed, length-prefixed segments (4-byte big-endian length + payload per segment).

Batch `/recommend` is adaptive: with `DECK_ENGINE_THREADS=1` items fan out across the Rust engine pool; with a value greater than 1 the whole batch goes through a single native C++ batch FFI call that parallelizes internally.

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
