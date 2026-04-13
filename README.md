# Deck Service

A high-performance HTTP service for **Project Sekai** deck recommendation, powered by a C++ computation engine with a Rust/Axum HTTP layer.

## Architecture

```
HTTP Request → Axum (Rust) → JSON FFI Bridge → C++ Engine → JSON Response
```

- **Rust + Axum** — async HTTP server with JSON request/response handling
- **C FFI Bridge** (`cpp_bridge/`) — translates between Rust and the C++ engine via JSON strings
- **C++ Engine** (`_cpp_src/`) — [sekai-deck-recommend-cpp](https://github.com/moe-sekai/sekai-deck-recommend-cpp), the core recommendation algorithms
- **Zig** — used as the C++ cross-compiler toolchain (via `zig c++`)

The output binary is **fully statically linked** (musl libc) with no runtime dependencies, ideal for minimal container images.

## Prerequisites

- [Rust](https://rustup.rs/) ≥ 1.85 (edition 2024)
- [Zig](https://ziglang.org/download/) ≥ 0.14
- [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) — for cross-compilation

```bash
cargo install cargo-zigbuild
rustup target add x86_64-unknown-linux-musl
```

## Building

### Clone C++ source

The C++ source is gitignored. Clone it into `_cpp_src/`:

```bash
git clone https://github.com/moe-sekai/sekai-deck-recommend-cpp _cpp_src
cd _cpp_src && git submodule update --init --recursive && cd ..
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
# Required: path to the C++ engine's static data directory
export DECK_DATA_DIR=/path/to/_cpp_src/data

# Optional: listen address (default: 0.0.0.0:3000)
export BIND_ADDR=0.0.0.0:3000

# Optional: log level control
export RUST_LOG=deck_service=info

# Optional: warn when waiting for the engine lock too long (ms)
export DECK_LOCK_WARN_MS=1000

# Optional: fail fast if the shared engine lock cannot be acquired in time (ms)
export DECK_LOCK_TIMEOUT_MS=30000

# Optional: warn when a single engine call runs too long (ms)
export DECK_ENGINE_WARN_MS=10000

# Optional: inject a default recommend timeout_ms when the request does not provide one
export DECK_RECOMMEND_TIMEOUT_MS=15000

./deck-service
```

## Docker

```bash
docker build -t deck-service .
docker run -p 3000:3000 -v /path/to/data:/data -e DECK_DATA_DIR=/data deck-service
```

The Docker image uses `scratch` as the base (only the static binary), resulting in a ~4 MB image.

## API Reference

All endpoints accept and return JSON. The body size limit is 1000 MB.

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
| `target` | `string` | Optimization target (`"score"`, `"event_point"`, `"mysekai_event_point"`) |
| `algorithm` | `string` | Search algorithm (`"dfs"`, `"sa"`, `"ga"`) |
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
| `filter_other_unit` | `bool` | Filter cards from other units |
| `fixed_cards` | `int[]` | Cards that must be in the deck |
| `fixed_characters` | `int[]` | Characters that must be in the deck |
| `sa_options` | `object` | Simulated annealing parameters |
| `ga_options` | `object` | Genetic algorithm parameters |

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
| `BIND_ADDR` | `0.0.0.0:3000` | HTTP server listen address |
| `RUST_LOG` | `deck_service=info` | Tracing log filter |
| `DECK_LOCK_WARN_MS` | `1000` | Warn threshold for waiting on the shared engine mutex |
| `DECK_LOCK_TIMEOUT_MS` | `30000` | Fail-fast timeout for acquiring the shared engine mutex |
| `DECK_ENGINE_WARN_MS` | `10000` | Warn threshold for a single FFI/engine operation |
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
- waiting for the shared engine lock
- entering/leaving each FFI call
- per-item progress inside batch recommend

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
│   └── deck_recommend_c.cpp  # C bridge implementation (nlohmann/json)
├── build.rs             # Build script: zig c++ compilation
├── build.zig            # Zig build file (alternative, unused by cargo)
├── Cargo.toml
├── Dockerfile
└── _cpp_src/            # (gitignored) cloned C++ engine source
```

## License

LGPL-2.1 — see [LICENSE](LICENSE).

## Credits

- [xfl03/sekai-calculator](https://github.com/xfl03/sekai-calculator) — original algorithms and implementation
- [NeuraXmy/sekai-deck-recommend-cpp](https://github.com/NeuraXmy/sekai-deck-recommend-cpp) — C++ engine original implementation
- [moe-sekai/sekai-deck-recommend-cpp](https://github.com/moe-sekai/sekai-deck-recommend-cpp) — current C++ engine maintaining
