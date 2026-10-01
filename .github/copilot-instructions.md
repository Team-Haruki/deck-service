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
- Fetch the pinned source with `./scripts/prepare-cpp-engine.sh` (commit from `cpp-engine.ref`, with submodules, into `_cpp_src/`)
- Cross-compile: `cargo zigbuild --target x86_64-unknown-linux-musl`
- Docker: multi-stage build → `scratch` image (static musl binary); `/data` read-only static data, `/cache` (uid 65532) holds the RL seed cache via `DECK_RL_SEED_CACHE_FILE`

## Architecture

```
handlers.rs → bridge.rs → ffi.rs → cpp_bridge/ → _cpp_src/ (C++ engine)
```

FFI boundary uses JSON strings. `DeckRecommend` handle is `Send` (not `Sync`), concurrent access goes through `EnginePool` in `state.rs` (reader/writer lock pattern with `parking_lot`).

## Concurrency

- `EnginePool` manages N engine instances (default: `min(cpu_count, 4)`)
- `checkout`: acquires one slot for recommend calls (concurrent readers)
- `checkout_all`: exclusive access for broadcast updates (masterdata/musicmetas)
- `UserdataCache` holds userdata payloads (LRU bounded by `DECK_USERDATA_CACHE_MAX_ENTRIES`/`_MAX_BYTES`/`_TTL_SECONDS`, entries tagged by the regions that used them); each engine slot tracks loaded hashes to skip redundant FFI calls; exclusive updates invalidate through `invalidate_userdata` with `UserdataInvalidation::Region`

## Key Files

- `models.rs` — request/response types (mirrors upstream Python API)
- `state.rs` — `AppState`, `EnginePool`, `UserdataCache`
- `masterdata.rs` — region-aware masterdata directory resolution
- `cpp_bridge/deck_recommend_c.cpp` — C bridge using yyjson
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
