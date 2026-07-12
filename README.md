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
export DECK_DATA_DIR=/path/to/_cpp_src/data

# Optional: preload region masterdata at startup
export DECK_MASTERDATA_BASE_DIR=/path/to/masterdata-root

# Optional: poll mounted masterdata for changes (ms, default: 300000; 0 disables)
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
is loaded by startup env vars or update endpoints.

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
docker run -p 3000:3000 -v /path/to/data:/data -e DECK_DATA_DIR=/data deck-service
```

The Docker image uses `scratch` as the base (only the static binary), resulting in a ~4 MB image.
By default it builds against `Team-Haruki/sekai-deck-recommend-cpp` branch
`master` at commit `b2387b7f09e5a420c9bfee9ece8903b345dd39cd`;
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
  "event_id": 123,
  "world_bloom_character_id": 1,
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

### Update Masterdata (from directory)

```
POST /update/masterdata
{ "base_dir": "/path/to/masterdata", "region": "jp" }
→ { "status": "ok" }
```

### Update Masterdata (from JSON)

```
POST /update/masterdata/json
{ "data": { "cards.json": "...", "skills.json": "..." }, "region": "jp" }
→ { "status": "ok" }
```

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
| `DECK_MASTERDATA_DIR` / `DECK_MASTERDATA_BASE_DIR` | unset | Base directory used to preload region masterdata on startup |
| `DECK_MASTERDATA_REGIONS` | `jp,en,cn,tw,kr` | CSV list of regions to preload masterdata for |
| `DECK_MUSICMETAS_DIR` / `DECK_MUSICMETAS_BASE_DIR` | masterdata base, then `/app/data` | Base directory used to preload region music metas on startup |
| `DECK_MUSICMETAS_REGIONS` | `jp,en,cn,tw,kr` | CSV list of regions to preload music metas for |
| `DECK_MUSICMETAS_FILE_<REGION>` | unset | Explicit music metas file path for one region, e.g. `DECK_MUSICMETAS_FILE_JP` |
| `BIND_ADDR` | `0.0.0.0:3000` | HTTP server listen address |
| `RUST_LOG` | `deck_service=info` | Tracing log filter |
| `DECK_LOCK_WARN_MS` | `1000` | Warn threshold for waiting on an engine pool slot |
| `DECK_LOCK_TIMEOUT_MS` | `30000` | Fail-fast timeout for acquiring an engine pool slot |
| `DECK_ENGINE_WARN_MS` | `10000` | Warn threshold for a single FFI/engine operation |
| `DECK_ENGINE_POOL_SIZE` | `min(cpu_count, 4)` | Number of engine instances used for concurrent recommends |
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

## Cloud Guide

For production/cloud integration, see [docs/cloud-call-guide.md](docs/cloud-call-guide.md).

## Local Hybrid Benchmark

When `_cpp_src/` and region masterdata are available locally, you can run the built-in hybrid benchmark:

```bash
cargo run --release --bin hybrid_bench
```

The benchmark uses local snapshot JSONs under `../metadata/`, loads `music_metas` from `/tmp/music_metas_{region}.json`, and compares:

- `dfs` — exact search with timeout bound
- `ga` — pure genetic search
- `dfs_ga` — DFS warmup to seed GA
- `rl` — learned policy + remembered seeds + seeded GA refine

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
