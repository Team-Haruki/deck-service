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

## Key Files

- `models.rs` — request/response types (mirrors upstream Python API)
- `state.rs` — `AppState`, `EnginePool`, `UserdataCache`
- `masterdata.rs` — region-aware masterdata directory resolution
- `cpp_bridge/deck_recommend_c.cpp` — C bridge using nlohmann/json
- `build.zig` — compiles C++ sources and C bridge into the static archive for Zig-backed targets
- `build.rs` — Cargo glue for path resolution and link metadata

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
  - Claude (any 4.x): `Co-authored-by: Claude Opus 4.7 <noreply@anthropic.com>` (substitute the actual model, e.g. `Claude Sonnet 4.6`, `Claude Haiku 4.5`)
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

Use the standardized workflow layout in `.github/workflows`:

- `ci.yml` runs on `main` pushes, pull requests targeting `main`, and manual dispatch.
- Rust CI order: `cargo fmt --all -- --check`, `cargo check --locked --all-targets`, `cargo clippy --locked --all-targets -- -D warnings`, then `cargo test --locked`.
- `release.yml` is the standard release build entrypoint. It runs on `v*` tags and manual dispatch, builds release artifacts, uploads them with `actions/upload-artifact`, and publishes GitHub Release assets on tag pushes.
- `docker.yml` is the standard Docker entrypoint. It runs on `main` pushes, `v*` tags, PRs that touch Docker/build inputs, and manual dispatch. PRs build only; non-PR runs push GHCR images with lowercase image names and Docker metadata tags.

Workflow maintenance rules:

- Keep workflow filenames and top-level names aligned: `CI`, `Release`, `Docker`, and optional package-specific names.
- Use `actions/checkout@v6`, `actions/setup-go@v6`, `actions/upload-artifact@v7`, `actions/download-artifact@v8`, `softprops/action-gh-release@v3`, and current Docker actions (`setup-buildx@v4`, `login@v4`, `metadata@v6`, `build-push@v7`).
- Keep `permissions` minimal: `contents: read` for CI/Docker build-only work, `contents: write` for release publishing, and `packages: write` only when pushing container images.
- Use workflow `concurrency` keyed by workflow name and ref, with release jobs using `release-${{ github.ref_name }}` and `cancel-in-progress: false`.
- Do not reintroduce legacy workflow names such as `rust-ci.yml`, `build.yml`, `release-build.yml`, `docker-build.yml`, or `docker-release.yml` unless a package-specific workflow already exists and is intentionally preserved.
