# Deck Service

A high-performance HTTP service for **Project Sekai** deck recommendation, powered by a C++ computation engine with a Rust/Axum HTTP layer.

## Architecture

```
HTTP Request → Axum (Rust) → JSON FFI Bridge → C++ Engine → JSON Response
```

- **Rust + Axum** — async HTTP server with JSON request/response handling
- **C FFI Bridge** (`cpp_bridge/`) — translates between Rust and the C++ engine via JSON strings
- **C++ Engine** (`_cpp_src/`) — Team Haruki's maintained [sekai-deck-recommend-cpp](https://github.com/Team-Haruki/sekai-deck-recommend-cpp) fork, consumed here as core C++ sources through a C/Rust FFI bridge
- **Zig** — builds the C++ bridge/engine archive for static/cross targets through `build.zig`

The output binary is **fully statically linked** (musl libc) with no runtime dependencies, ideal for minimal container images.

## Prerequisites

For deck-service itself:

- [Rust](https://rustup.rs/) ≥ 1.85 (edition 2024)
- [Zig](https://ziglang.org/download/) ≥ 0.14
- [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) — for cross-compilation
- `g++` / libstdc++ headers — for Linux GNU host builds (`cargo build`)

```bash
cargo install cargo-zigbuild
rustup target add x86_64-unknown-linux-musl
```

The upstream C++ repository also publishes Python and WebAssembly packages. Its
own packaging workflows use CMake, Python 3.10+, `uv`, and `emsdk`, but those are
not required for building this HTTP service unless you are working directly on
the upstream package targets.

## Building

### Clone C++ source

The C++ source is gitignored. The current deck-service build tracks Team
Haruki's default upstream branch (`master`). Clone it into `_cpp_src/` with
submodules:

```bash
git clone --recursive https://github.com/Team-Haruki/sekai-deck-recommend-cpp.git _cpp_src
```

For an existing checkout:

```bash
git -C _cpp_src fetch origin master
git -C _cpp_src checkout master
git -C _cpp_src submodule update --init --recursive
```

You can also keep the C++ repository elsewhere and point builds at it:

```bash
export DECK_CPP_SRC=/path/to/sekai-deck-recommend-cpp
```

### Native build (macOS / Linux)

```bash
cargo build --release
```

### Cross-compile to Linux x86_64 (static musl)

```bash
cargo zigbuild --release --target x86_64-unknown-linux-musl
```

Output: `target/x86_64-unknown-linux-musl/release/deck-service` (~4 MB, statically linked ELF)

## Running

```bash
# Required: path to the C++ engine's static data directory.
# This is the upstream static data/, not runtime masterdata/music metas.
# Treated as read-only static data; the RL seed cache goes to DECK_RL_SEED_CACHE_FILE.
export DECK_DATA_DIR=/path/to/_cpp_src/data

# Optional: writable RL seed cache file (unset -> $DECK_DATA_DIR/rl_seed_cache.tsv;
# DECK_RL_SEED_CACHE_DISABLE=1 turns persistence off)
export DECK_RL_SEED_CACHE_FILE=/path/to/cache/rl_seed_cache.tsv

# Preferred: pull master data (and music metas) from the Haruki master registry.
# Plain http on the private network; replaces the mounted masterdata volume.
export DECK_REGISTRY_URL=http://100.76.159.97:9998

# Legacy/deprecated: preload region masterdata from a mounted directory at startup
export DECK_MASTERDATA_BASE_DIR=/path/to/masterdata-root

# Optional (legacy directory path): poll mounted masterdata for changes (ms, default: 300000; 0 disables)
export DECK_MASTERDATA_REFRESH_MS=300000

# Optional: preload music metas at startup
export DECK_MUSICMETAS_BASE_DIR=/path/to/music-metas-root

# Optional: listen address (default: 0.0.0.0:3000)
export BIND_ADDR=0.0.0.0:3000

# Optional: log level control
export RUST_LOG=deck_service=info

# Optional: warn when waiting for an engine slot too long (ms)
export DECK_LOCK_WARN_MS=1000

# Optional: fail fast if an engine slot cannot be acquired in time (ms)
export DECK_LOCK_TIMEOUT_MS=30000

# Optional: warn when a single engine call runs too long (ms)
export DECK_ENGINE_WARN_MS=10000

# Optional: number of C++ engine instances kept in the pool (default: min(cpu_count, 4))
export DECK_ENGINE_POOL_SIZE=4

# Optional: inject a default recommend timeout_ms when the request does not provide one
export DECK_RECOMMEND_TIMEOUT_MS=15000

./deck-service
```

Masterdata, music metas, and userdata are application/runtime inputs. They are
not bundled by the upstream WebAssembly npm package, and deck-service follows
the same model: static engine data comes from `_cpp_src/data`, while region data
is loaded by startup env vars or update endpoints. Prefer `DECK_REGISTRY_URL`;
the masterdata directory path (`DECK_MASTERDATA_BASE_DIR` and
`POST /update/masterdata`) is legacy and deprecated.

## Upstream Packages

The upstream README documents these package targets:

- Python package: `haruki-sekai-deck-recommend-cpp`, installable with `uv add`, `uv pip install`, or `pip install`.
- Source install: `git clone --recursive`, then `uv pip install -e . -v` or `pip install -e . -v`.
- WebAssembly npm package: `npm/haruki-sekai-deck-recommend-cpp`, built with activated `emsdk` using `emcmake cmake -S . -B build_wasm -G Ninja -DCMAKE_BUILD_TYPE=Release`, `cmake --build build_wasm -j`, then `npm pack`.

deck-service does not import the Python or npm packages; it links the same C++
engine sources directly through `cpp_bridge/`.

## Docker

```bash
docker build -t deck-service .
docker run -p 3000:3000 -v deck-rl-cache:/cache deck-service
```

The container has three different mounts, each with its own role:

- `/data` is the engine's static data, baked into the image and read-only
  (`DECK_DATA_DIR=/data`).
- `/cache` holds the RL seed cache (`DECK_RL_SEED_CACHE_FILE=/cache/rl_seed_cache.tsv`)
  and must be writable by uid 65532: use a named volume (seeded with the right
  ownership from the image) or a host directory after `chown 65532:65532`. With a
  read-only root filesystem, mount a named volume or tmpfs there. At startup the
  service logs `RL seed cache enabled`, a not-writable warning, or `disabled`.
- Master data comes from `DECK_REGISTRY_URL`, or from a legacy masterdata
  directory mounted wherever `DECK_MASTERDATA_BASE_DIR` points.

The Docker image uses `scratch` as the base (only the static binary), resulting in a ~4 MB image.
By default it builds against `Team-Haruki/sekai-deck-recommend-cpp` branch
`master` at commit `05111fd203202b48efe61ebcbaf926e2b4d4dbb8`;
override `DECK_CPP_REPO`, `DECK_CPP_BRANCH`, or `DECK_CPP_REF` as build args if
you intentionally need a different engine checkout.

## API Reference

Most endpoints accept and return JSON. `/cache_userdata` and batch `/recommend`
use the binary protocol described below. The body size limit is 1000 MB.

### Health Check

```
GET /health
→ "ok"
```

### Recommend Deck

```
POST /recommend
Content-Type: application/json
```

**Required fields:**

| Field | Type | Description |
| --- | --- | --- |
| `region` | `string` | Game region (e.g. `"jp"`, `"tw"`, `"en"`) |
| `live_type` | `string` | Live type (`"multi"`, `"solo"`, `"cheerful"`, `"challenge"`) |
| `music_id` | `int` | Music ID |
| `music_diff` | `string` | Difficulty (`"easy"`, `"normal"`, `"hard"`, `"expert"`, `"master"`, `"append"`) |

**Optional fields:**

| Field | Type | Description |
| --- | --- | --- |
| `target` | `string` | Optimization target (`"score"`, `"skill"`, `"power"`, `"bonus"`) |
| `algorithm` | `string` | Search algorithm (`"dfs"`, `"ga"`, `"dfs_ga"`, `"rl"`) |
| `userdata_hash` | `string` | Hash returned by `/cache_userdata` for server-side cached userdata |
| `user_data_file_path` | `string` | Path to user data file |
| `user_data_str` | `string` | User data as inline JSON string |
| `event_id` | `int` | Event ID |
| `event_attr` | `string` | Event attribute |
| `event_unit` | `string` | Event unit |
| `event_type` | `string` | Event type |
| `world_bloom_event_turn` | `int` | World bloom event turn |
| `world_bloom_finale_turn` | `int` | Simulated World Bloom finale turn (`2` or `3`) |
| `world_bloom_character_id` | `int` | World bloom character ID |
| `challenge_live_character_id` | `int` | Challenge live character ID |
| `limit` | `int` | Max number of result decks |
| `member` | `int` | Deck member count |
| `timeout_ms` | `int` | Timeout in milliseconds |
| `rarity_*_config` | `object` | Card config per rarity (`1`, `2`, `3`, `birthday`, `4`) |
| `single_card_configs` | `array` | Per-card overrides |
| `support_master_max` | `bool` | Treat support cards as max master rank |
| `support_skill_max` | `bool` | Treat support cards as max skill level |
| `filter_other_unit` | `bool` | Filter cards from other units |
| `fixed_cards` | `int[]` | Cards that must be in the deck |
| `fixed_characters` | `int[]` | Characters that must be in the deck |
| `forced_leader_character_id` | `int` | Force the leader card's character ID |
| `target_bonus_list` | `int[]` | Optional exact bonus targets for `target: "bonus"` |
| `custom_bonus_character_ids` | `int[]` | Optional custom mixed-event bonus character IDs |
| `custom_bonus_attr` | `string` | Optional custom mixed-event bonus attribute |
| `custom_bonus_character_support_units` | `object` | Optional virtual singer support-unit constraints keyed by character ID |
| `skill_reference_choose_strategy` | `string` | Skill reference strategy passed through to the engine |
| `keep_after_training_state` | `bool` | Keep cards' existing after-training state |
| `multi_live_teammate_score_up` | `int` | Multi-live teammate score-up value |
| `multi_live_teammate_power` | `int` | Multi-live teammate power value |
| `best_skill_as_leader` | `bool` | Prefer best skill as leader |
| `multi_live_score_up_lower_bound` | `float` | Lower bound for multi-live score-up |
| `skill_order_choose_strategy` | `string` | Skill order strategy passed through to the engine |
| `specific_skill_order` | `int[]` | Explicit skill order |
| `sa_options` | `object` | Simulated annealing parameters |
| `ga_options` | `object` | Genetic algorithm parameters for `ga`, `dfs_ga`, and `rl` |

**Response:**

```json
{
  "decks": [
    {
      "score": 1234567,
      "live_score": 1200000,
      "mysekai_event_point": 0,
      "total_power": 280000,
      "base_power": 250000,
      "area_item_bonus_power": 15000,
      "character_bonus_power": 10000,
      "honor_bonus_power": 3000,
      "fixture_bonus_power": 1000,
      "gate_bonus_power": 1000,
      "event_bonus_rate": 250.0,
      "support_deck_bonus_rate": 10.0,
      "multi_live_score_up": 1.0,
      "cards": [
        {
          "card_id": 123,
          "total_power": 56000,
          "base_power": 50000,
          "event_bonus_rate": 50.0,
          "master_rank": 5,
          "level": 60,
          "skill_level": 4,
          "skill_score_up": 120.0,
          "skill_life_recovery": 0.0,
          "episode1_read": true,
          "episode2_read": true,
          "after_training": true,
          "default_image": "special_training",
          "has_canvas_bonus": false
        }
      ]
    }
  ]
}
```

### Cache Userdata

```
POST /cache_userdata
Content-Type: application/octet-stream
```

Request body: zstd-compressed binary protocol with exactly one userdata JSON
segment.

Response:

```json
{ "userdata_hash": "..." }
```

Use the returned `userdata_hash` in later `/recommend` requests to avoid
resending large userdata payloads.

The server bounds cached payloads by count, bytes and idle time (see
[User data cache limits](#user-data-cache-limits)). A hash is tagged with a region the first time a request uses
it; a master data or music metas update for one region drops the hashes tagged
with that region and the hashes no request has used yet. Hashes used only with
other regions stay cached. A dropped hash makes the next request fail with
`400 User data not found for userdata_hash`; call `/cache_userdata` again.

### Batch Recommend

```
POST /recommend
Content-Type: application/octet-stream
```

Request body: zstd-compressed binary protocol with exactly one JSON segment:

```json
{
  "region": "jp",
  "userdata_hash": "...",
  "batch_options": [
    {
      "live_type": "multi",
      "music_id": 74,
      "music_diff": "expert",
      "algorithm": "ga",
      "timeout_ms": 15000
    }
  ]
}
```

Response: JSON array of per-item results with `alg`, `cost_time`, `wait_time`,
and either `result` or `error`.

Successful `result` objects include `cost_ms`, the C++ search-algorithm wall
time in milliseconds. It excludes option/userdata parsing and result
conversion. The outer `cost_time` remains the complete per-item engine-call
time in seconds for backward compatibility.

Batch execution adapts to the configured concurrency model. With the default
`DECK_ENGINE_THREADS=1`, items use separate instances from the Rust engine
pool. When `DECK_ENGINE_THREADS` is greater than 1, the batch uses one engine
checkout and the C++ engine's persistent worker pool, which prevents nested
parallel regions from oversubscribing the CPU.

### Binary Protocol

For `application/octet-stream` endpoints, concatenate one or more segments as
`4-byte big-endian length + payload`, then zstd-compress the whole framed byte
stream. `/cache_userdata` and batch `/recommend` currently expect exactly one
segment.

### World Bloom Support Cards

```
POST /world_bloom/support_cards
{
  "region": "jp",
  "userdata_hash": "...",
  "world_bloom_finale_turn": 3,
  "forced_leader_character_id": 1,
  "support_master_max": true,
  "support_skill_max": true
}
```

Response: JSON array of support cards sorted by support bonus descending:

```json
[
  { "card_id": 123, "bonus": 12.5 }
]
```

### Update Masterdata (registry / legacy directory)

```
POST /update/masterdata/registry
{ "region": "jp", "content_hash": "<optional: the registry contentHash the caller already knows>" }
→ { "status": "ok", "region": "jp", "contentHash": "…", "gitCommit": "…", "dataVersion": "…", "reloaded": true|false }
Pulls the region's current manifest from `DECK_REGISTRY_URL`; a matching
`content_hash` short-circuits without a round trip, an unchanged manifest only
re-checks music metas (conditional GET), a changed `contentHash` reloads.
502 when the registry cannot be reached, 503 when `DECK_REGISTRY_URL` is unset.
502 `registry master data has empty key tables: <names>` when any key table
(`areaItemLevels`, `areaItems`, `areas`, `cardEpisodes`, `cards`, `cardRarities`,
`characterRanks`, `gameCharacters`, `gameCharacterUnits`, `honors`,
`masterLessons`, `musicDifficulties`, `musics`, `musicVocals`, `skills`) is not
a non-empty JSON array; the region keeps its previously loaded data.

GET /state/masterdata
→ { "registryUrl": "http://…" | null, "regions": { "jp": { "contentHash", "gitCommit", "dataVersion", "loadedAt", "source": "registry", "musicMetasDigest", "missingOptionalKeys": ["ingameNotes", …] } } }
Only registry-loaded regions are listed; directory-loaded regions have no
version identity and are omitted. `missingOptionalKeys` (sorted) is present
only when the loaded manifest lacked optional engine keys; the load still
succeeds, the engine treats those tables as empty, and a warn log
(`missing_optional_count`, `missing_optional`) is emitted. The engine's own
stderr line `master data key not found: <key>` keeps printing; the Rust warn is
the structured one. Missing World Link finale tables
(`worldBloomSupportDeckUnitEventLimitedBonuses` and friends) make finale
requests fail per request rather than compute a zero bonus.

POST /update/masterdata   (Deprecated: legacy directory path)
{ "base_dir": "/path/to/masterdata", "region": "jp" }
→ { "status": "ok", "deprecated": true, "replacement": "/update/masterdata/registry" }
Deprecated; use `POST /update/masterdata/registry`. Removed in the release after
the registry fetcher (deck-service #19) ships. Status codes and error bodies are
unchanged; a successful response carries `Deprecation: true` and
`Link: </update/masterdata/registry>; rel="successor-version"`, and each call
logs a warning.
```

### Update Masterdata (from JSON)

```
POST /update/masterdata/json
{ "data": { "cards.json": "...", "skills.json": "..." }, "region": "jp" }
→ { "status": "ok" }
400 `masterdata lacks required keys: <names>` when a required key is absent.
400 `masterdata key tables are empty: <names>` when a key table (same list as
the registry path) is not a non-empty JSON array. Keys are normalised like the
engine does (`master/cards.json` → `cards`) before the check; the engine is
not touched on either 400. Missing optional keys never fail the push; they are
logged as a warning (`missing_optional_count`, `missing_optional`).
```

The directory path (`POST /update/masterdata`) is not audited: its files are
read inside the engine.

### Update Music Metas (from file)

```
POST /update/musicmetas
{ "file_path": "/path/to/music_metas.json", "region": "jp" }
→ { "status": "ok" }
```

### Update Music Metas (from string)

```
POST /update/musicmetas/string
{ "data": "{...json content...}", "region": "jp" }
→ { "status": "ok" }
```

## Environment Variables

| Variable | Default | Description |
| --- | --- | --- |
| `DECK_DATA_DIR` | (relative to binary) | Path to the C++ engine's static data directory |
| `DECK_RL_SEED_CACHE_FILE` | image: `/cache/rl_seed_cache.tsv`; binary: unset (→ `$DECK_DATA_DIR/rl_seed_cache.tsv`) | RL seed cache file read by the engine; its directory must be writable. Checked and logged at startup |
| `DECK_RL_SEED_CACHE_DISABLE` | unset | Set to the literal `1` to disable RL seed cache persistence |
| `DECK_REGISTRY_URL` | unset | Master registry base URL (plain http). When set, the regions in `DECK_REGISTRY_REGIONS` are loaded from `GET /v1/master/{region}/current` + `blob/{sha256}` and `GET /v1/metas/{region}/music_metas.json` instead of the directory variables below, which then only apply to regions not listed there |
| `DECK_REGISTRY_REGIONS` | `jp,en,cn,tw,kr` | CSV of regions served by the registry |
| `DECK_REGISTRY_REFRESH_MS` | `300000` | Poll interval for the registry manifest (`0` disables; a reload happens only when `contentHash` changes, music metas use `If-None-Match`) |
| `DECK_REGISTRY_FETCH_CONCURRENCY` | `8` | Parallel blob downloads per region load |
| `DECK_REGISTRY_TIMEOUT_MS` | `30000` | Per-request timeout against the registry |
| `DECK_MASTERDATA_DIR` / `DECK_MASTERDATA_BASE_DIR` | unset | Legacy (deprecated directory path): base directory used to preload region masterdata on startup |
| `DECK_MASTERDATA_REGIONS` | `jp,en,cn,tw,kr` | Legacy (deprecated directory path): CSV list of regions to preload masterdata for |
| `DECK_MASTERDATA_REFRESH_MS` | `300000` | Legacy (deprecated directory path): poll interval for mounted masterdata changes (`0` disables) |
| `DECK_MUSICMETAS_DIR` / `DECK_MUSICMETAS_BASE_DIR` | masterdata base, then `/app/data` | Base directory used to preload region music metas on startup |
| `DECK_MUSICMETAS_REGIONS` | `jp,en,cn,tw,kr` | CSV list of regions to preload music metas for |
| `DECK_MUSICMETAS_FILE_<REGION>` | unset | Explicit music metas file path for one region, e.g. `DECK_MUSICMETAS_FILE_JP` |
| `BIND_ADDR` | `0.0.0.0:3000` | HTTP server listen address |
| `RUST_LOG` | `deck_service=info` | Tracing log filter |
| `DECK_LOCK_WARN_MS` | `1000` | Warn threshold for waiting on an engine pool slot |
| `DECK_LOCK_TIMEOUT_MS` | `30000` | Fail-fast timeout for acquiring an engine pool slot |
| `DECK_ENGINE_WARN_MS` | `10000` | Warn threshold for a single FFI/engine operation |
| `DECK_ENGINE_POOL_SIZE` | `min(cpu_count, 4)` | Number of engine instances used for concurrent recommends |
| `DECK_USERDATA_CACHE_MAX_BYTES` | `268435456` | Userdata cache byte budget, including estimated entry metadata |
| `DECK_USERDATA_CACHE_MAX_ENTRIES` | `128` | Maximum cached userdata payloads (LRU) |
| `DECK_USERDATA_CACHE_TTL_SECONDS` | `1800` | Idle time after which a cached payload is dropped |
| `DECK_ENGINE_THREADS` | `1` | C++ engine-internal parallelism; keep `pool size × engine threads` within the available CPU count |
| `DECK_RECOMMEND_TIMEOUT_MS` | unset | Default `timeout_ms` injected into recommend requests when missing |

## Debugging Hung Requests

When investigating a suspected deadlock or long stall, start the service with:

```bash
export RUST_LOG=deck_service=debug
export DECK_LOCK_WARN_MS=500
export DECK_LOCK_TIMEOUT_MS=5000
export DECK_ENGINE_WARN_MS=3000
export DECK_RECOMMEND_TIMEOUT_MS=8000
```

This enables per-request `op_id` logs around:

- request admission
- waiting for an engine pool slot
- entering/leaving each FFI call
- per-item progress inside batch recommend
- per-item lock wait / engine execution time inside batch recommend

## Project Structure

```
deck-service/
├── src/
│   ├── main.rs          # Axum router & server entry point
│   ├── handlers.rs      # HTTP route handlers
│   ├── models.rs        # Request/response serde types
│   ├── bridge.rs        # Safe Rust wrapper around C FFI
│   ├── ffi.rs           # Raw unsafe extern "C" bindings
│   ├── state.rs         # Shared application state (Mutex<Engine>)
│   ├── registry.rs      # Master registry client (manifest, blobs, music metas)
│   ├── masterdata.rs    # Legacy masterdata directory resolution (deprecated path)
│   ├── masterdata_audit.rs # Master data key checks (required, key tables, optional)
│   └── error.rs         # AppError → HTTP response mapping
├── cpp_bridge/
│   ├── deck_recommend_c.h    # C API header
│   └── deck_recommend_c.cpp  # C bridge implementation (yyjson)
├── build.rs             # Cargo glue for Zig-built C++ static library
├── build.zig            # Zig build file for the C++ bridge/engine archive
├── cpp_sources.txt      # C++ engine source list shared by build tooling
├── Cargo.toml
├── Dockerfile
└── _cpp_src/            # (gitignored) cloned Team Haruki C++ engine source
```

## License

LGPL-2.1 — see [LICENSE](LICENSE).

## Credits

- [xfl03/sekai-calculator](https://github.com/xfl03/sekai-calculator) — original algorithms and implementation
- [NeuraXmy/sekai-deck-recommend-cpp](https://github.com/NeuraXmy/sekai-deck-recommend-cpp) — C++ engine original implementation
- [Team-Haruki/sekai-deck-recommend-cpp](https://github.com/Team-Haruki/sekai-deck-recommend-cpp) — current C++ engine maintenance, Python package, and WebAssembly/npm target

## User data cache limits

The Rust replay cache uses LRU eviction, a byte budget and an idle TTL. Defaults:

- `DECK_USERDATA_CACHE_MAX_BYTES=268435456` (256 MiB, including estimated entry metadata).
- `DECK_USERDATA_CACHE_MAX_ENTRIES=128`.
- `DECK_USERDATA_CACHE_TTL_SECONDS=1800` (30 minutes since the last cache access).

Idle entries are removed on access and by a 60-second sweep. Each payload is limited to 32 MiB; compressed protocol decoding is limited to 64 MiB. Cache eviction leaves in-flight `Arc` references valid, so these limits describe retained cache data, not total process RSS. Each engine tracks at most 64 loaded hashes, matching the C++ cache capacity.

`GET /cache/stats` reports counts, estimated bytes, limits and evictions, without payloads or user hashes. An expired or evicted hash returns `User data not found for userdata_hash`; clients must upload the snapshot again before retrying. Haruki Cloud handles this automatically.
