#include "deck_recommend_c.h"

#include "deck-recommend/event-deck-recommend.h"
#include "deck-recommend/challenge-live-deck-recommend.h"
#include "deck-recommend/mysekai-deck-recommend.h"
#include "data-provider/static-data.h"
#include "data-provider/master-data.h"
#include "data-provider/music-metas.h"
#include "data-provider/user-data.h"
#include "data-provider/data-provider.h"
#include "common/enum-maps.h"
#include "common/collection-utils.h"
#include "card-information/card-calculator.h"
#include "deck-recommend/base-deck-recommend.h"
#include "deck-recommend/deck-result-update.h"
#include "deck-information/deck-calculator.h"
#include "deck-information/deck-service.h"
#include "live-score/live-calculator.h"
#include "common/parallel-utils.h"

#include <algorithm>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <deque>
#include <limits>
#include <map>
#include <memory>
#include <mutex>
#include <optional>
#include <set>
#include <shared_mutex>
#include <stdexcept>
#include <string>
#include <string_view>
#include <tuple>
#include <type_traits>
#include <unordered_map>
#include <vector>

namespace {
constexpr int kMaxRecommendTimeoutMs = 5000;
}

// ---- helpers ----

static char* alloc_cstr(const std::string& s) {
    auto result = std::make_unique_for_overwrite<char[]>(s.size() + 1);
    std::memcpy(result.get(), s.c_str(), s.size() + 1);
    return result.release();
}

static char* alloc_cstr(const std::string& s, size_t* len_out) {
    if (len_out) {
        *len_out = s.size();
    }
    return alloc_cstr(s);
}

static char* alloc_error(const std::string& msg) {
    return alloc_cstr(msg);
}

static size_t nullable_cstr_size(const char* value) {
    return value ? std::string_view(value).size() : 0;
}

template <typename Writer>
static std::string write_json_with_pool(Writer&& writer, const std::string& error_prefix) {
    size_t capacity = 4096;
    while (true) {
        const size_t word_count =
            (capacity + sizeof(std::max_align_t) - 1) / sizeof(std::max_align_t);
        std::vector<std::max_align_t> storage(word_count);
        yyjson_alc allocator{};
        if (!yyjson_alc_pool_init(
                &allocator,
                storage.data(),
                storage.size() * sizeof(std::max_align_t)
            )) {
            throw std::runtime_error(error_prefix + ": failed to initialize JSON memory pool");
        }

        size_t len = 0;
        yyjson_write_err err{};
        if (char* out = writer(&allocator, &len, &err)) {
            return std::string(out, len);
        }
        if (err.code != YYJSON_WRITE_ERROR_MEMORY_ALLOCATION) {
            const std::string message = err.msg ? err.msg : "unknown error";
            throw std::runtime_error(error_prefix + ": " + message);
        }
        if (capacity > std::numeric_limits<size_t>::max() / 2) {
            throw std::runtime_error(error_prefix + ": JSON output is too large");
        }
        capacity *= 2;
    }
}

static std::string hash_userdata_payload(std::string_view payload) {
    uint64_t hash = 14695981039346656037ull;
    for (unsigned char ch : payload) {
        hash ^= static_cast<uint64_t>(ch);
        hash *= 1099511628211ull;
    }

    char buffer[17];
    std::snprintf(buffer, sizeof(buffer), "%016llx", static_cast<unsigned long long>(hash));
    return std::string(buffer);
}

static bool ends_with_json_suffix(std::string_view value) {
    if (value.size() < 5) {
        return false;
    }
    auto suffix = value.substr(value.size() - 5);
    return (suffix[0] == '.')
        && (suffix[1] == 'j' || suffix[1] == 'J')
        && (suffix[2] == 's' || suffix[2] == 'S')
        && (suffix[3] == 'o' || suffix[3] == 'O')
        && (suffix[4] == 'n' || suffix[4] == 'N');
}

static std::string normalize_masterdata_key(std::string key) {
    auto last_sep = key.find_last_of("/\\");
    if (last_sep != std::string::npos) {
        key = key.substr(last_sep + 1);
    }
    if (ends_with_json_suffix(key)) {
        key.resize(key.size() - 5);
    }
    return key;
}

static std::string json_read_error_message(const yyjson_read_err& err) {
    return err.msg ? std::string(err.msg) : std::string("unknown error");
}

static json_doc parse_json_bytes(const char* data, size_t len, const std::string& source) {
    if (!data || len == 0) {
        throw std::runtime_error("Failed to parse " + source + ": empty JSON input");
    }

    yyjson_read_err err{};
    std::string padded(data, len);
    padded.resize(len + YYJSON_PADDING_SIZE);
    yyjson_doc* parsed = yyjson_read_opts(
        padded.data(),
        len,
        YYJSON_READ_NOFLAG,
        nullptr,
        &err
    );
    if (!parsed) {
        throw std::runtime_error(
            "Failed to parse " + source + " at position " + std::to_string(err.pos) +
            ": " + json_read_error_message(err)
        );
    }
    return json_doc(parsed);
}

static std::string dump_json(const json_view& value) {
    return write_json_with_pool(
        [&](yyjson_alc* allocator, size_t* len, yyjson_write_err* err) {
            return yyjson_val_write_opts(
                value.raw(),
                YYJSON_WRITE_NOFLAG,
                allocator,
                len,
                err
            );
        },
        "Failed to serialize JSON value"
    );
}

static std::string extract_json_string_or_dump(const json_view& value) {
    if (value.is_string()) {
        return value.get<std::string>();
    }
    return dump_json(value);
}

template <typename T>
static std::optional<T> json_opt(const json_view& opts, const char* key) {
    if (!opts.contains(key) || opts[key].is_null()) {
        return std::nullopt;
    }
    return opts[key].get<T>();
}

static std::string context_value_or_json(
    const json_view& opts,
    const char* key,
    const char* forced_value,
    size_t forced_value_len,
    const char* required_name
) {
    if (forced_value && forced_value_len > 0) {
        return std::string(forced_value, forced_value_len);
    }
    if (!opts.contains(key) || !opts[key].is_string()) {
        throw std::invalid_argument(std::string(required_name) + " is required.");
    }
    return opts[key].get<std::string>();
}

static std::string json_type_name(const json_view& value) {
    if (value.is_null()) return "null";
    if (value.is_object()) return "object";
    if (value.is_array()) return "array";
    if (value.is_string()) return "string";
    if (value.is_number_integer()) return "integer";
    return "other";
}

class MutableJsonDoc {
public:
    MutableJsonDoc() : doc_(yyjson_mut_doc_new(nullptr)) {
        if (!doc_) {
            throw std::runtime_error("Failed to allocate mutable JSON document.");
        }
    }

    MutableJsonDoc(const MutableJsonDoc&) = delete;
    MutableJsonDoc& operator=(const MutableJsonDoc&) = delete;

    ~MutableJsonDoc() {
        yyjson_mut_doc_free(doc_);
    }

    yyjson_mut_doc* get() const {
        return doc_;
    }

private:
    yyjson_mut_doc* doc_ = nullptr;
};

template <typename T, std::enable_if_t<std::is_integral_v<T> && !std::is_same_v<T, bool>, int> = 0>
static void json_add(yyjson_mut_doc* doc, yyjson_mut_val* obj, const char* key, T value) {
    if (!yyjson_mut_obj_add_int(doc, obj, key, value)) {
        throw std::runtime_error(std::string("Failed to add JSON integer field: ") + key);
    }
}

static void json_add(yyjson_mut_doc* doc, yyjson_mut_val* obj, const char* key, double value) {
    if (!yyjson_mut_obj_add_real(doc, obj, key, value)) {
        throw std::runtime_error(std::string("Failed to add JSON real field: ") + key);
    }
}

static void json_add(yyjson_mut_doc* doc, yyjson_mut_val* obj, const char* key, bool value) {
    if (!yyjson_mut_obj_add_bool(doc, obj, key, value)) {
        throw std::runtime_error(std::string("Failed to add JSON bool field: ") + key);
    }
}

static void json_add(yyjson_mut_doc* doc, yyjson_mut_val* obj, const char* key, const std::string& value) {
    if (!yyjson_mut_obj_add_strncpy(doc, obj, key, value.data(), value.size())) {
        throw std::runtime_error(std::string("Failed to add JSON string field: ") + key);
    }
}

static void json_add_value(yyjson_mut_doc* doc, yyjson_mut_val* obj, const char* key, yyjson_mut_val* value) {
    if (!yyjson_mut_obj_add_val(doc, obj, key, value)) {
        throw std::runtime_error(std::string("Failed to add JSON value field: ") + key);
    }
}

static void json_array_append(yyjson_mut_val* arr, yyjson_mut_val* value) {
    if (!yyjson_mut_arr_append(arr, value)) {
        throw std::runtime_error("Failed to append JSON array value.");
    }
}

static std::string dump_mutable_json(yyjson_mut_val* value) {
    return write_json_with_pool(
        [&](yyjson_alc* allocator, size_t* len, yyjson_write_err* err) {
            return yyjson_mut_val_write_opts(
                value,
                YYJSON_WRITE_NOFLAG,
                allocator,
                len,
                err
            );
        },
        "Failed to serialize JSON output"
    );
}

static yyjson_mut_val* json_object(yyjson_mut_doc* doc) {
    yyjson_mut_val* obj = yyjson_mut_obj(doc);
    if (!obj) {
        throw std::runtime_error("Failed to allocate JSON object.");
    }
    return obj;
}

static yyjson_mut_val* json_array(yyjson_mut_doc* doc) {
    yyjson_mut_val* arr = yyjson_mut_arr(doc);
    if (!arr) {
        throw std::runtime_error("Failed to allocate JSON array.");
    }
    return arr;
}

static std::string require_string_field(const json_view& opts, const std::string& key) {
    if (!opts.contains(key) || !opts[key].is_string()) {
        throw std::invalid_argument(key + " is required.");
    }
    return opts[key].get<std::string>();
}

static int require_int_field(const json_view& opts, const std::string& key) {
    if (!opts.contains(key) || !opts[key].is_number_integer()) {
        throw std::invalid_argument(key + " is required.");
    }
    return opts[key].get<int>();
}

static bool has_int_field(const json_view& opts, const std::string& key) {
    return opts.contains(key) && !opts[key].is_null() && opts[key].is_number_integer();
}

static yyjson_mut_val* deck_power_to_json(yyjson_mut_doc* doc, const DeckPowerDetail& power) {
    yyjson_mut_val* j = json_object(doc);
    json_add(doc, j, "base", power.base);
    json_add(doc, j, "areaItemBonus", power.areaItemBonus);
    json_add(doc, j, "characterBonus", power.characterBonus);
    json_add(doc, j, "honorBonus", power.honorBonus);
    json_add(doc, j, "fixtureBonus", power.fixtureBonus);
    json_add(doc, j, "gateBonus", power.gateBonus);
    json_add(doc, j, "total", power.total);
    return j;
}

static yyjson_mut_val* deck_card_power_to_json(yyjson_mut_doc* doc, const DeckCardPowerDetail& power) {
    yyjson_mut_val* j = json_object(doc);
    json_add(doc, j, "base", power.base);
    json_add(doc, j, "areaItemBonus", power.areaItemBonus);
    json_add(doc, j, "characterBonus", power.characterBonus);
    json_add(doc, j, "fixtureBonus", power.fixtureBonus);
    json_add(doc, j, "gateBonus", power.gateBonus);
    json_add(doc, j, "total", power.total);
    return j;
}

static yyjson_mut_val* deck_card_skill_to_json(yyjson_mut_doc* doc, const DeckCardSkillDetail& skill) {
    yyjson_mut_val* j = json_object(doc);
    json_add(doc, j, "scoreUp", skill.scoreUp);
    json_add(doc, j, "lifeRecovery", skill.lifeRecovery);
    return j;
}

static yyjson_mut_val* deck_detail_to_json(yyjson_mut_doc* doc, const DeckDetail& deckDetail) {
    yyjson_mut_val* j = json_object(doc);
    json_add_value(doc, j, "power", deck_power_to_json(doc, deckDetail.power));
    if (deckDetail.eventBonus.has_value()) {
        json_add(doc, j, "eventBonus", deckDetail.eventBonus.value());
    }
    if (deckDetail.supportDeckBonus.has_value()) {
        json_add(doc, j, "supportDeckBonus", deckDetail.supportDeckBonus.value());
    }

    yyjson_mut_val* cards = json_array(doc);
    for (const auto& card : deckDetail.cards) {
        yyjson_mut_val* cj = json_object(doc);
        json_add(doc, cj, "cardId", card.cardId);
        json_add(doc, cj, "level", card.level);
        json_add(doc, cj, "skillLevel", card.skillLevel);
        json_add(doc, cj, "masterRank", card.masterRank);
        json_add_value(doc, cj, "power", deck_card_power_to_json(doc, card.power));
        json_add_value(doc, cj, "skill", deck_card_skill_to_json(doc, card.skill));
        json_array_append(cards, cj);
    }
    json_add_value(doc, j, "cards", cards);
    return j;
}

static yyjson_mut_val* live_detail_to_json(yyjson_mut_doc* doc, const LiveDetail& liveDetail) {
    yyjson_mut_val* j = json_object(doc);
    json_add(doc, j, "score", liveDetail.score);
    json_add(doc, j, "time", liveDetail.time);
    json_add(doc, j, "life", liveDetail.life);
    json_add(doc, j, "tap", liveDetail.tap);
    if (liveDetail.deck.has_value()) {
        json_add_value(doc, j, "deck", deck_detail_to_json(doc, liveDetail.deck.value()));
    }
    return j;
}

static std::optional<std::vector<LiveSkill>> parse_live_skills(const json_view& opts) {
    if (!opts.contains("skills") || opts["skills"].is_null()) {
        return std::nullopt;
    }
    if (!opts["skills"].is_array()) {
        throw std::invalid_argument("skills must be an array.");
    }

    std::vector<LiveSkill> liveSkills{};
    for (const auto& item : opts["skills"]) {
        if (!item.is_object()) {
            throw std::invalid_argument("skills entries must be objects.");
        }
        LiveSkill liveSkill{};
        if (item.contains("seq") && item["seq"].is_number_integer()) {
            liveSkill.seq = item["seq"].get<int>();
        }
        if (item.contains("cardId") && item["cardId"].is_number_integer()) {
            liveSkill.cardId = item["cardId"].get<int>();
        } else if (item.contains("card_id") && item["card_id"].is_number_integer()) {
            liveSkill.cardId = item["card_id"].get<int>();
        } else {
            throw std::invalid_argument("skills entries require cardId.");
        }
        liveSkills.push_back(liveSkill);
    }
    return liveSkills;
}

static void apply_card_config(CardConfig& dst, const json_view& src) {
    if (src.contains("disable") && !src["disable"].is_null()) {
        dst.disable = src["disable"].get<bool>();
    }
    if (src.contains("level_max") && !src["level_max"].is_null()) {
        dst.rankMax = src["level_max"].get<bool>();
    }
    if (src.contains("episode_read") && !src["episode_read"].is_null()) {
        dst.episodeRead = src["episode_read"].get<bool>();
    }
    if (src.contains("master_max") && !src["master_max"].is_null()) {
        dst.masterMax = src["master_max"].get<bool>();
    }
    if (src.contains("skill_max") && !src["skill_max"].is_null()) {
        dst.skillMax = src["skill_max"].get<bool>();
    }
    if (src.contains("canvas") && !src["canvas"].is_null()) {
        dst.canvas = src["canvas"].get<bool>();
    }
    if (src.contains("level") && !src["level"].is_null()) {
        dst.level = src["level"].get<int>();
    }
    if (src.contains("skill_level") && !src["skill_level"].is_null()) {
        dst.skillLevel = src["skill_level"].get<int>();
    }
    if (src.contains("master_rank") && !src["master_rank"].is_null()) {
        dst.masterRank = src["master_rank"].get<int>();
    }
    if (src.contains("episode_read_count") && !src["episode_read_count"].is_null()) {
        dst.episodeReadCount = src["episode_read_count"].get<int>();
    }
}

static yyjson_mut_val* recommend_deck_to_json(yyjson_mut_doc* doc, const RecommendDeck& deck) {
    yyjson_mut_val* dj = json_object(doc);
    json_add(doc, dj, "score", deck.score);
    json_add(doc, dj, "live_score", deck.liveScore);
    json_add(doc, dj, "mysekai_event_point", deck.mysekaiEventPoint);
    json_add(doc, dj, "total_power", deck.power.total);
    json_add(doc, dj, "base_power", deck.power.base);
    json_add(doc, dj, "area_item_bonus_power", deck.power.areaItemBonus);
    json_add(doc, dj, "character_bonus_power", deck.power.characterBonus);
    json_add(doc, dj, "honor_bonus_power", deck.power.honorBonus);
    json_add(doc, dj, "fixture_bonus_power", deck.power.fixtureBonus);
    json_add(doc, dj, "gate_bonus_power", deck.power.gateBonus);
    json_add(doc, dj, "event_bonus_rate", deck.eventBonus.value_or(0.0));
    json_add(doc, dj, "support_deck_bonus_rate", deck.supportDeckBonus.value_or(0.0));
    json_add(doc, dj, "multi_live_score_up", deck.multiLiveScoreUp);

    if (deck.supportDeckCards.has_value()) {
        yyjson_mut_val* support_cards_json = json_array(doc);
        for (const auto& supportCard : deck.supportDeckCards.value()) {
            yyjson_mut_val* scj = json_object(doc);
            json_add(doc, scj, "card_id", supportCard.cardId);
            json_add(doc, scj, "bonus", supportCard.supportDeckBonus.value_or(0.0));
            json_add(doc, scj, "skill_level", supportCard.skillLevel);
            json_add(doc, scj, "master_rank", supportCard.masterRank);
            json_add(doc, scj, "level", supportCard.level);
            json_add(doc, scj, "after_training", supportCard.afterTraining);
            json_add(doc, scj, "default_image", mappedEnumToString(EnumMap::defaultImage, supportCard.defaultImage));
            json_array_append(support_cards_json, scj);
        }
        json_add_value(doc, dj, "support_deck_cards", support_cards_json);
    }

    yyjson_mut_val* cards_json = json_array(doc);
    for (const auto& card : deck.cards) {
        yyjson_mut_val* cj = json_object(doc);
        json_add(doc, cj, "card_id", card.cardId);
        json_add(doc, cj, "total_power", card.power.total);
        json_add(doc, cj, "base_power", card.power.base);
        json_add(doc, cj, "area_item_bonus_power", card.power.areaItemBonus);
        json_add(doc, cj, "character_bonus_power", card.power.characterBonus);
        json_add(doc, cj, "fixture_bonus_power", card.power.fixtureBonus);
        json_add(doc, cj, "gate_bonus_power", card.power.gateBonus);
        json_add(doc, cj, "event_bonus_rate", card.eventBonus.value_or(0.0));
        json_add(doc, cj, "master_rank", card.masterRank);
        json_add(doc, cj, "level", card.level);
        json_add(doc, cj, "skill_level", card.skillLevel);
        json_add(doc, cj, "skill_score_up", card.skill.scoreUp);
        json_add(doc, cj, "skill_life_recovery", card.skill.lifeRecovery);
        json_add(doc, cj, "episode1_read", card.episode1Read);
        json_add(doc, cj, "episode2_read", card.episode2Read);
        json_add(doc, cj, "after_training", card.afterTraining);
        json_add(doc, cj, "default_image", mappedEnumToString(EnumMap::defaultImage, card.defaultImage));
        json_add(doc, cj, "has_canvas_bonus", card.hasCanvasBonus);
        json_array_append(cards_json, cj);
    }
    json_add_value(doc, dj, "cards", cards_json);
    return dj;
}

// ---- region map ----

static const std::map<std::string, Region> REGION_MAP = {
    {"jp", Region::JP}, {"tw", Region::TW}, {"en", Region::EN},
    {"kr", Region::KR}, {"cn", Region::CN},
};

// ---- validation sets ----

static const std::set<std::string> VALID_TARGETS = {"score","skill","power","bonus"};
static const std::set<std::string> VALID_ALGORITHMS = {
    "sa", "dfs", "ga", "dfs_ga", "dfs-ga", "rl"
};
static const std::set<std::string> VALID_MUSIC_DIFFS = {"easy","normal","hard","expert","master","append"};
static const std::set<std::string> VALID_LIVE_TYPES = {"multi","solo","challenge","cheerful","auto","mysekai","challenge_auto"};
static const std::set<std::string> VALID_UNIT_TYPES = {"light_sound","idol","street","theme_park","school_refusal","piapro"};
static const std::set<std::string> VALID_EVENT_ATTRS = {"mysterious","cool","pure","cute","happy"};
static const std::set<std::string> VALID_EVENT_TYPES = {"marathon","cheerful_carnival","world_bloom"};
static const std::set<std::string> VALID_SKILL_REF_STRATEGIES = {"average","max","min"};
static const std::set<std::string> VALID_SKILL_ORDER_STRATEGIES = {"average","max","min","specific"};

// ---- process-level shared cache for read-only region data ----
// MasterData and MusicMetas are immutable after load. Sharing one shared_ptr
// per region across every SekaiDeckRecommendC handle avoids paying N*|data|
// when the engine pool spins up multiple handles.

namespace {
class SharedRegionDataStore {
    mutable std::shared_mutex mutex_;
    std::map<Region, std::shared_ptr<MasterData>> region_masterdata_;
    std::map<Region, std::shared_ptr<MusicMetas>> region_musicmetas_;

public:
    void set_masterdata(Region region, std::shared_ptr<MasterData> data) {
        std::unique_lock lock(mutex_);
        region_masterdata_[region] = std::move(data);
    }

    void set_musicmetas(Region region, std::shared_ptr<MusicMetas> data) {
        std::unique_lock lock(mutex_);
        region_musicmetas_[region] = std::move(data);
    }

    std::shared_ptr<MasterData> get_masterdata(Region region) const {
        std::shared_lock lock(mutex_);
        auto it = region_masterdata_.find(region);
        return it == region_masterdata_.end() ? nullptr : it->second;
    }

    std::shared_ptr<MusicMetas> get_musicmetas(Region region) const {
        std::shared_lock lock(mutex_);
        auto it = region_musicmetas_.find(region);
        return it == region_musicmetas_.end() ? nullptr : it->second;
    }
};

SharedRegionDataStore& shared_region_data_store() {
    static SharedRegionDataStore instance;
    return instance;
}

class SharedUserdataStore {
    mutable std::shared_mutex mutex_;
    std::unordered_map<std::string, std::shared_ptr<const UserData>> entries_;
    std::deque<std::string> order_;

    static constexpr std::size_t max_entries = 64;

public:
    void remember(const std::string& userdata_hash, const std::shared_ptr<UserData>& userdata) {
        auto snapshot = std::make_shared<const UserData>(*userdata);

        std::unique_lock lock(mutex_);
        if (!entries_.count(userdata_hash)) {
            order_.push_back(userdata_hash);
        }
        entries_[userdata_hash] = std::move(snapshot);

        while (order_.size() > max_entries) {
            auto oldest = order_.front();
            order_.pop_front();
            entries_.erase(oldest);
        }
    }

    std::shared_ptr<UserData> clone(const std::string& userdata_hash) const {
        std::shared_lock lock(mutex_);
        auto it = entries_.find(userdata_hash);
        if (it == entries_.end()) {
            return nullptr;
        }
        return std::make_shared<UserData>(*it->second);
    }
};

SharedUserdataStore& shared_userdata_store() {
    static SharedUserdataStore instance;
    return instance;
}
}

// ---- internal SekaiDeckRecommend wrapper (same logic as pybind11/wasm versions) ----

class SekaiDeckRecommendC {
    std::unordered_map<std::string, std::shared_ptr<UserData>> userdata_cache;
    std::deque<std::string> userdata_cache_order;

    static constexpr std::size_t max_userdata_cache_entries = 64;

    void remember_userdata(
        const std::string& userdata_hash,
        const std::shared_ptr<UserData>& userdata,
        bool share_with_process_cache = true
    ) {
        if (!userdata_cache.count(userdata_hash)) {
            userdata_cache_order.push_back(userdata_hash);
        }
        userdata_cache[userdata_hash] = userdata;

        while (userdata_cache_order.size() > max_userdata_cache_entries) {
            auto oldest = userdata_cache_order.front();
            userdata_cache_order.pop_front();
            userdata_cache.erase(oldest);
        }

        if (share_with_process_cache) {
            shared_userdata_store().remember(userdata_hash, userdata);
        }
    }

    std::shared_ptr<UserData> attach_shared_userdata(const std::string& userdata_hash) {
        auto userdata = shared_userdata_store().clone(userdata_hash);
        if (!userdata) {
            throw std::invalid_argument("User data not found for userdata_hash: " + userdata_hash);
        }
        remember_userdata(userdata_hash, userdata, false);
        return userdata;
    }

    std::shared_ptr<UserData> resolve_userdata(
        const json_view& opts,
        const char* forced_userdata_hash = nullptr,
        size_t forced_userdata_hash_len = 0
    ) {
        if (forced_userdata_hash && forced_userdata_hash_len > 0) {
            std::string userdata_hash(forced_userdata_hash, forced_userdata_hash_len);
            auto it = userdata_cache.find(userdata_hash);
            if (it == userdata_cache.end()) {
                return attach_shared_userdata(userdata_hash);
            }
            return it->second;
        }

        if (opts.contains("userdata_hash") && !opts["userdata_hash"].is_null()) {
            if (!opts["userdata_hash"].is_string()) {
                throw std::invalid_argument("userdata_hash must be a string.");
            }

            std::string userdata_hash = opts["userdata_hash"].get<std::string>();
            auto it = userdata_cache.find(userdata_hash);
            if (it == userdata_cache.end()) {
                return attach_shared_userdata(userdata_hash);
            }
            return it->second;
        }

        auto userdata = std::make_shared<UserData>();
        if (opts.contains("user_data_file_path") && opts["user_data_file_path"].is_string()) {
            userdata->loadFromFile(opts["user_data_file_path"].get<std::string>());
            return userdata;
        }

        const char* user_data_key = nullptr;
        if (opts.contains("user_data_str") && !opts["user_data_str"].is_null()) {
            user_data_key = "user_data_str";
        } else if (opts.contains("user_data") && !opts["user_data"].is_null()) {
            user_data_key = "user_data";
        }
        if (user_data_key != nullptr) {
            auto userdata_str = extract_json_string_or_dump(opts[user_data_key]);
            userdata->loadFromString(userdata_str);
            remember_userdata(hash_userdata_payload(userdata_str), userdata);
            return userdata;
        }

        throw std::invalid_argument(
            "Either userdata_hash, user_data_file_path, user_data_str or user_data is required."
        );
    }

    DataProvider build_data_provider(
        const json_view& opts,
        bool require_musicmetas,
        const char* forced_region = nullptr,
        size_t forced_region_len = 0,
        const char* forced_userdata_hash = nullptr,
        size_t forced_userdata_hash_len = 0
    ) {
        std::string region_str = context_value_or_json(
            opts,
            "region",
            forced_region,
            forced_region_len,
            "region"
        );
        if (!REGION_MAP.count(region_str)) {
            throw std::invalid_argument("Invalid region: " + region_str);
        }
        Region region = REGION_MAP.at(region_str);

        auto userdata = resolve_userdata(opts, forced_userdata_hash, forced_userdata_hash_len);

        auto masterdata = shared_region_data_store().get_masterdata(region);
        if (!masterdata) {
            throw std::invalid_argument("Master data not found for region: " + region_str);
        }
        auto musicmetas = shared_region_data_store().get_musicmetas(region);
        if (require_musicmetas && !musicmetas) {
            throw std::invalid_argument("Music metas not found for region: " + region_str);
        }
        if (!musicmetas) {
            musicmetas = std::make_shared<MusicMetas>();
        }
        return DataProvider{region, masterdata, userdata, musicmetas};
    }

    std::vector<UserCard> resolve_fixed_deck_cards(DataProvider& dp, const json_view& opts, const std::string& mode) {
        DeckService deckService(dp);
        if (mode == "challenge") {
            auto challengeDeck = deckService.getChallengeLiveSoloDeck(require_int_field(opts, "character_id"));
            return deckService.getChallengeLiveSoloDeckCards(challengeDeck);
        }

        if (mode == "deck") {
            auto userDeck = deckService.getDeck(require_int_field(opts, "deck_id"));
            return deckService.getDeckCards(userDeck);
        }

        if (mode == "live_full") {
            if (has_int_field(opts, "character_id")) {
                auto challengeDeck = deckService.getChallengeLiveSoloDeck(opts["character_id"].get<int>());
                return deckService.getChallengeLiveSoloDeckCards(challengeDeck);
            }
            if (has_int_field(opts, "deck_id")) {
                auto userDeck = deckService.getDeck(opts["deck_id"].get<int>());
                return deckService.getDeckCards(userDeck);
            }
            throw std::invalid_argument("Either deck_id or character_id is required.");
        }

        throw std::invalid_argument("Invalid calculate mode: " + mode);
    }

    DeckDetail calculate_fixed_deck_detail(DataProvider& dp, const std::vector<UserCard>& deckCards) {
        if (deckCards.empty()) {
            throw std::invalid_argument("fixed deck contains no cards.");
        }

        dp.init();

        CardCalculator cardCalculator(dp);
        DeckCalculator deckCalculator(dp);
        std::unordered_map<int, CardConfig> config{};
        std::unordered_map<int, CardConfig> singleCardConfig{};
        auto cardDetails = cardCalculator.batchGetCardDetail(deckCards, config, singleCardConfig);
        if (cardDetails.size() != deckCards.size()) {
            throw std::runtime_error("Failed to calculate all cards in fixed deck.");
        }

        std::vector<const CardDetail*> cardDetailPtrs{};
        cardDetailPtrs.reserve(cardDetails.size());
        for (const auto& cardDetail : cardDetails) {
            cardDetailPtrs.push_back(&cardDetail);
        }

        std::map<int, std::vector<SupportDeckCard>> supportCards{};
        auto deckDetails = deckCalculator.getDeckDetailByCards(
            cardDetailPtrs,
            supportCards,
            deckCalculator.getHonorBonusPower(),
            std::nullopt,
            std::nullopt,
            SkillReferenceChooseStrategy::Max,
            false,
            false
        );
        if (deckDetails.empty()) {
            throw std::runtime_error("Failed to calculate fixed deck detail.");
        }
        return deckDetails.front();
    }

    static int parse_custom_bonus_character_id_key(
        const std::string& cid_key,
        const json_view& value
    ) {
        try {
            size_t parsed = 0;
            int cid = std::stoi(cid_key, &parsed);
            if (parsed != cid_key.size()) {
                throw std::invalid_argument("invalid key");
            }
            return cid;
        } catch (const std::exception&) {
            throw std::invalid_argument(
                "Invalid custom bonus support unit character ID key: " + cid_key +
                " (value type: " + json_type_name(value) + ")"
            );
        }
    }

    static void apply_custom_bonus_attribute(DeckRecommendConfig& config, const json_view& opts) {
        if (auto value = json_opt<std::string>(opts, "custom_bonus_attr")) {
            if (!VALID_EVENT_ATTRS.count(*value)) {
                throw std::invalid_argument("Invalid custom bonus attr: " + *value);
            }
            config.customBonusAttr = mapEnum(EnumMap::attr, *value);
        }
    }

    static void apply_custom_bonus_characters(DeckRecommendConfig& config, const json_view& opts) {
        if (auto value = json_opt<std::vector<int>>(opts, "custom_bonus_character_ids")) {
            std::set<int> unique_cids;
            std::vector<int> cids;
            for (int cid : *value) {
                if (cid < 1 || cid > 26) {
                    throw std::invalid_argument("Invalid custom bonus character ID: " + std::to_string(cid));
                }
                if (unique_cids.count(cid)) {
                    throw std::invalid_argument("Duplicate custom bonus character ID: " + std::to_string(cid));
                }
                unique_cids.insert(cid);
                cids.push_back(cid);
            }
            config.customBonusCharacterIds = cids;
        }
    }

    static void apply_custom_bonus_support_units(DeckRecommendConfig& config, const json_view& opts) {
        if (!opts.contains("custom_bonus_character_support_units")
            || opts["custom_bonus_character_support_units"].is_null()) {
            return;
        }
        json_view support_units_json = opts["custom_bonus_character_support_units"];
        if (!support_units_json.is_object()) {
            throw std::invalid_argument("custom_bonus_character_support_units must be an object.");
        }

        std::unordered_map<int, int> support_units;
        yyjson_obj_iter iter = yyjson_obj_iter_with(support_units_json.raw());
        yyjson_val* key = nullptr;
        while ((key = yyjson_obj_iter_next(&iter))) {
            const char* raw_cid_key = yyjson_get_str(key);
            const std::string cid_key = raw_cid_key ? std::string(raw_cid_key) : std::string();
            json_view value(yyjson_obj_iter_get_val(key));
            int cid = parse_custom_bonus_character_id_key(cid_key, value);
            std::string unit_name = value.get<std::string>();
            if (cid < 21 || cid > 26) {
                throw std::invalid_argument(
                    "custom bonus support unit is only valid for virtual singer characters."
                );
            }
            if (!VALID_UNIT_TYPES.count(unit_name) || unit_name == "piapro") {
                throw std::invalid_argument("Invalid custom bonus support unit: " + unit_name);
            }
            if (!config.customBonusCharacterIds.has_value()
                || std::find(
                    config.customBonusCharacterIds->begin(),
                    config.customBonusCharacterIds->end(),
                    cid
                ) == config.customBonusCharacterIds->end()) {
                throw std::invalid_argument(
                    "custom bonus support unit character ID must be included in custom_bonus_character_ids: "
                    + std::to_string(cid)
                );
            }
            support_units[cid] = mapEnum(EnumMap::unit, unit_name);
        }
        config.customBonusSupportUnits = support_units;
    }

    static void apply_custom_bonus_options(DeckRecommendConfig& config, const json_view& opts) {
        apply_custom_bonus_attribute(config, opts);
        apply_custom_bonus_characters(config, opts);
        apply_custom_bonus_support_units(config, opts);
    }

    struct LiveContext {
        int type;
        bool is_mysekai;
        bool is_challenge;
    };

    static Region resolve_region(const std::string& region_str) {
        if (!REGION_MAP.count(region_str)) {
            throw std::invalid_argument("Invalid region: " + region_str);
        }
        return REGION_MAP.at(region_str);
    }

    static LiveContext resolve_live_context(const json_view& opts) {
        if (!opts.contains("live_type") || !opts["live_type"].is_string()) {
            throw std::invalid_argument("live_type is required.");
        }
        std::string live_type_str = opts["live_type"].get<std::string>();
        if (!VALID_LIVE_TYPES.count(live_type_str)) {
            throw std::invalid_argument("Invalid live type: " + live_type_str);
        }
        bool is_mysekai = live_type_str == "mysekai";
        int live_type = is_mysekai
            ? mapEnum(EnumMap::liveType, "multi")
            : mapEnum(EnumMap::liveType, live_type_str);
        return {live_type, is_mysekai, Enums::LiveType::isChallenge(live_type)};
    }

    static int resolve_world_bloom_fake_event_id(
        const json_view& opts,
        const DataProvider& dp
    ) {
        int turn = opts["world_bloom_event_turn"].get<int>();
        if (turn < 1 || turn > 3) {
            throw std::invalid_argument("Invalid world bloom event turn.");
        }
        if (turn == 3) {
            auto character_id = json_opt<int>(opts, "world_bloom_character_id");
            if (!character_id.has_value()) {
                throw std::invalid_argument(
                    "world_bloom_character_id is required for world bloom 3 fake event."
                );
            }
            int part = dp.masterData->getWorldBloom3PartByCharacterId(*character_id);
            return dp.masterData->getWorldBloomFakeEventId(turn, part);
        }

        auto event_unit = json_opt<std::string>(opts, "event_unit");
        if (!event_unit.has_value()) {
            throw std::invalid_argument("event_unit is required for world bloom fake event.");
        }
        if (!VALID_UNIT_TYPES.count(*event_unit)) {
            throw std::invalid_argument("Invalid event unit: " + *event_unit);
        }
        return dp.masterData->getWorldBloomFakeEventId(
            turn,
            mapEnum(EnumMap::unit, *event_unit)
        );
    }

    static int resolve_fake_event_id(const json_view& opts, const DataProvider& dp) {
        std::string event_type = json_opt<std::string>(opts, "event_type").value_or("marathon");
        if (!VALID_EVENT_TYPES.count(event_type)) {
            throw std::invalid_argument("Invalid event type: " + event_type);
        }
        if (json_opt<int>(opts, "world_bloom_event_turn").has_value()) {
            return resolve_world_bloom_fake_event_id(opts, dp);
        }

        bool has_event_attr = opts.contains("event_attr");
        bool has_event_unit = opts.contains("event_unit");
        auto event_type_enum = mapEnum(EnumMap::eventType, event_type);
        if (!has_event_attr && !has_event_unit) {
            return dp.masterData->getNoEventFakeEventId(event_type_enum);
        }
        if (!has_event_attr || !has_event_unit) {
            throw std::invalid_argument("event_attr and event_unit must be specified together.");
        }
        std::string event_attr = opts["event_attr"].get<std::string>();
        std::string event_unit = opts["event_unit"].get<std::string>();
        if (!VALID_EVENT_ATTRS.count(event_attr)) {
            throw std::invalid_argument("Invalid event attr: " + event_attr);
        }
        if (!VALID_UNIT_TYPES.count(event_unit)) {
            throw std::invalid_argument("Invalid event unit: " + event_unit);
        }
        return dp.masterData->getUnitAttrFakeEventId(
            event_type_enum,
            mapEnum(EnumMap::unit, event_unit),
            mapEnum(EnumMap::attr, event_attr)
        );
    }

    static int resolve_event_id(
        const json_view& opts,
        const DataProvider& dp,
        bool is_challenge
    ) {
        auto event_id = json_opt<int>(opts, "event_id");
        if (!event_id.has_value()) {
            return is_challenge ? 0 : resolve_fake_event_id(opts, dp);
        }
        if (is_challenge) {
            throw std::invalid_argument("event_id is not valid for challenge live.");
        }
        findOrThrow(dp.masterData->events, [&](const Event& event) {
            return event.id == *event_id;
        }, "Event not found for eventId: " + std::to_string(*event_id));
        return *event_id;
    }

    static int resolve_challenge_character_id(const json_view& opts, bool is_challenge) {
        auto character_id = json_opt<int>(opts, "challenge_live_character_id");
        if (!character_id.has_value()) {
            if (is_challenge) {
                throw std::invalid_argument(
                    "challenge_live_character_id is required for challenge live."
                );
            }
            return 0;
        }
        if (*character_id < 1 || *character_id > 26) {
            throw std::invalid_argument("Invalid challenge character ID.");
        }
        return *character_id;
    }

    static int resolve_world_bloom_character_id(
        const json_view& opts,
        const DataProvider& dp,
        int event_id
    ) {
        auto character_id = json_opt<int>(opts, "world_bloom_character_id");
        if (!character_id.has_value()) {
            return 0;
        }
        if (*character_id < 1 || *character_id > 26) {
            throw std::invalid_argument("Invalid world bloom character ID.");
        }
        findOrThrow(dp.masterData->worldBlooms, [&](const WorldBloom& world_bloom) {
            return world_bloom.eventId == event_id
                && world_bloom.gameCharacterId == *character_id;
        }, std::string("World bloom chapter not found."));
        return *character_id;
    }

    static void apply_target_options(
        DeckRecommendConfig& config,
        const json_view& opts,
        bool is_mysekai
    ) {
        if (is_mysekai) {
            config.target = RecommendTarget::Mysekai;
        } else {
            std::string target = json_opt<std::string>(opts, "target").value_or("score");
            if (!VALID_TARGETS.count(target)) {
                throw std::invalid_argument("Invalid target: " + target);
            }
            if (target == "score") config.target = RecommendTarget::Score;
            if (target == "skill") config.target = RecommendTarget::Skill;
            if (target == "power") config.target = RecommendTarget::Power;
            if (target == "bonus") config.target = RecommendTarget::Bonus;
        }

        auto bonus_list = json_opt<std::vector<int>>(opts, "target_bonus_list");
        if (!bonus_list.has_value() || bonus_list->empty()) {
            return;
        }
        if (config.target != RecommendTarget::Bonus) {
            throw std::invalid_argument("target_bonus_list is only valid for bonus target.");
        }
        config.bonusList = *bonus_list;
    }

    static void apply_algorithm_options(
        DeckRecommendConfig& config,
        const json_view& opts,
        bool is_challenge
    ) {
        std::string algorithm = json_opt<std::string>(opts, "algorithm")
            .value_or(is_challenge ? "dfs" : "ga");
        if (!VALID_ALGORITHMS.count(algorithm)) {
            throw std::invalid_argument("Invalid algorithm: " + algorithm);
        }
        if (algorithm == "sa") config.algorithm = RecommendAlgorithm::SA;
        if (algorithm == "dfs") config.algorithm = RecommendAlgorithm::DFS;
        if (algorithm == "ga") config.algorithm = RecommendAlgorithm::GA;
        if (algorithm == "dfs_ga" || algorithm == "dfs-ga") {
            config.algorithm = RecommendAlgorithm::DFS_GA;
        }
        if (algorithm == "rl") config.algorithm = RecommendAlgorithm::RL;
    }

    static void apply_music_and_size_options(
        DeckRecommendConfig& config,
        const json_view& opts,
        const DataProvider& dp
    ) {
        config.musicId = require_int_field(opts, "music_id");
        std::string music_diff = require_string_field(opts, "music_diff");
        if (!VALID_MUSIC_DIFFS.count(music_diff)) {
            throw std::invalid_argument("Invalid music difficulty: " + music_diff);
        }
        config.musicDiff = mapEnum(EnumMap::musicDifficulty, music_diff);
        findOrThrow(dp.musicMetas->metas, [&](const MusicMeta& meta) {
            return meta.music_id == config.musicId && meta.difficulty == config.musicDiff;
        }, "Music meta not found for musicId: " + std::to_string(config.musicId));

        config.limit = json_opt<int>(opts, "limit").value_or(10);
        if (config.limit < 1) {
            throw std::invalid_argument("Invalid limit.");
        }
        config.member = json_opt<int>(opts, "member").value_or(5);
        if (config.member < 2 || config.member > 5) {
            throw std::invalid_argument("Invalid member count.");
        }
    }

    static void apply_fixed_card_options(
        DeckRecommendConfig& config,
        const json_view& opts,
        const DataProvider& dp
    ) {
        auto fixed_cards = json_opt<std::vector<int>>(opts, "fixed_cards");
        if (!fixed_cards.has_value()) {
            return;
        }
        config.fixedCards = *fixed_cards;
        if (static_cast<int>(config.fixedCards.size()) > config.member) {
            throw std::invalid_argument("Fixed cards size exceeds member count.");
        }
        for (int card_id : config.fixedCards) {
            findOrThrow(dp.masterData->cards, [&](const Card& card) {
                return card.id == card_id;
            }, "Invalid fixed card ID: " + std::to_string(card_id));
        }
    }

    static void apply_fixed_character_options(
        DeckRecommendConfig& config,
        const json_view& opts,
        bool is_challenge
    ) {
        auto fixed_characters = json_opt<std::vector<int>>(opts, "fixed_characters");
        if (!fixed_characters.has_value()) {
            return;
        }
        config.fixedCharacters = *fixed_characters;
        if (static_cast<int>(config.fixedCharacters.size()) > config.member) {
            throw std::invalid_argument("Fixed characters size exceeds member count.");
        }
        if (is_challenge) {
            throw std::invalid_argument("fixed_characters is not valid for challenge live.");
        }
        for (int character_id : config.fixedCharacters) {
            if (character_id < 1 || character_id > 26) {
                throw std::invalid_argument(
                    "Invalid fixed character ID: " + std::to_string(character_id)
                );
            }
        }
    }

    static void apply_forced_leader_option(DeckRecommendConfig& config, const json_view& opts) {
        auto character_id = json_opt<int>(opts, "forced_leader_character_id");
        if (!character_id.has_value()) {
            character_id = json_opt<int>(opts, "forcedLeaderCharacterId");
        }
        if (!character_id.has_value()) {
            return;
        }
        if (*character_id < 1 || *character_id > 26) {
            throw std::invalid_argument(
                "Invalid forced leader character ID: " + std::to_string(*character_id)
            );
        }
        config.forcedLeaderCharacterId = *character_id;
    }

    static void apply_skill_strategy_options(DeckRecommendConfig& config, const json_view& opts) {
        std::string reference_strategy = json_opt<std::string>(
            opts,
            "skill_reference_choose_strategy"
        ).value_or("average");
        if (!VALID_SKILL_REF_STRATEGIES.count(reference_strategy)) {
            throw std::invalid_argument("Invalid skill ref strategy: " + reference_strategy);
        }
        if (reference_strategy == "average") {
            config.skillReferenceChooseStrategy = SkillReferenceChooseStrategy::Average;
        }
        if (reference_strategy == "max") {
            config.skillReferenceChooseStrategy = SkillReferenceChooseStrategy::Max;
        }
        if (reference_strategy == "min") {
            config.skillReferenceChooseStrategy = SkillReferenceChooseStrategy::Min;
        }

        std::string order_strategy = json_opt<std::string>(
            opts,
            "skill_order_choose_strategy"
        ).value_or("average");
        if (!VALID_SKILL_ORDER_STRATEGIES.count(order_strategy)) {
            throw std::invalid_argument("Invalid skill order strategy: " + order_strategy);
        }
        if (order_strategy == "average") config.liveSkillOrder = LiveSkillOrder::average;
        if (order_strategy == "max") config.liveSkillOrder = LiveSkillOrder::best;
        if (order_strategy == "min") config.liveSkillOrder = LiveSkillOrder::worst;
        if (order_strategy == "specific") config.liveSkillOrder = LiveSkillOrder::specific;
        if (auto specific_order = json_opt<std::vector<int>>(opts, "specific_skill_order")) {
            config.specificSkillOrder = *specific_order;
        }
    }

    static void apply_multi_live_options(
        DeckRecommendConfig& config,
        const json_view& opts,
        int live_type
    ) {
        bool is_multi = Enums::LiveType::isMulti(live_type);
        if (auto score_up = json_opt<int>(opts, "multi_live_teammate_score_up")) {
            if (!is_multi) {
                throw std::invalid_argument(
                    "multi_live_teammate_score_up is only valid for multi live."
                );
            }
            config.multiTeammateScoreUp = *score_up;
        }
        if (auto power = json_opt<int>(opts, "multi_live_teammate_power")) {
            if (!is_multi) {
                throw std::invalid_argument(
                    "multi_live_teammate_power is only valid for multi live."
                );
            }
            config.multiTeammatePower = *power;
        }
        if (auto lower_bound = json_opt<double>(opts, "multi_live_score_up_lower_bound")) {
            if (!is_multi) {
                throw std::invalid_argument(
                    "multi_live_score_up_lower_bound is only valid for multi live."
                );
            }
            config.multiScoreUpLowerBound = *lower_bound;
        }
    }

    static void apply_card_options(DeckRecommendConfig& config, const json_view& opts) {
        for (const auto& rarity : {
            "rarity_1", "rarity_2", "rarity_3", "rarity_birthday", "rarity_4"
        }) {
            std::string key = std::string(rarity) + "_config";
            CardConfig card_config{};
            if (opts.contains(key) && opts[key].is_object()) {
                apply_card_config(card_config, opts[key]);
            }
            config.cardConfig[mapEnum(EnumMap::cardRarityType, rarity)] = card_config;
        }
        if (opts.contains("single_card_configs") && opts["single_card_configs"].is_array()) {
            for (const auto& item : opts["single_card_configs"]) {
                CardConfig card_config{};
                apply_card_config(card_config, item);
                config.singleCardConfig[item["card_id"].get<int>()] = card_config;
            }
        }
        if (auto support_master_max = json_opt<bool>(opts, "support_master_max")) {
            config.supportMasterMax = *support_master_max;
        }
        if (auto support_skill_max = json_opt<bool>(opts, "support_skill_max")) {
            config.supportSkillMax = *support_skill_max;
        }
    }

    static void apply_sa_options(DeckRecommendConfig& config, const json_view& opts) {
        if (!opts.contains("sa_options") || !opts["sa_options"].is_object()) {
            return;
        }
        const auto sa = opts["sa_options"];
        if (sa.contains("run_num")) config.saRunCount = sa["run_num"].get<int>();
        if (config.saRunCount < 1) {
            throw std::invalid_argument("Invalid sa run count: " + std::to_string(config.saRunCount));
        }
        if (sa.contains("seed")) config.saSeed = sa["seed"].get<int>();
        if (sa.contains("max_iter")) config.saMaxIter = sa["max_iter"].get<int>();
        if (config.saMaxIter < 1) {
            throw std::invalid_argument("Invalid sa max iter: " + std::to_string(config.saMaxIter));
        }
        if (sa.contains("max_no_improve_iter")) {
            config.saMaxIterNoImprove = sa["max_no_improve_iter"].get<int>();
        }
        if (config.saMaxIterNoImprove < 1) {
            throw std::invalid_argument(
                "Invalid sa max no improve iter: " + std::to_string(config.saMaxIterNoImprove)
            );
        }
        if (sa.contains("time_limit_ms")) config.saMaxTimeMs = sa["time_limit_ms"].get<int>();
        if (config.saMaxTimeMs < 0) {
            throw std::invalid_argument("Invalid sa max time ms: " + std::to_string(config.saMaxTimeMs));
        }
        if (sa.contains("start_temprature")) {
            config.saStartTemperature = sa["start_temprature"].get<double>();
        } else if (sa.contains("start_temperature")) {
            config.saStartTemperature = sa["start_temperature"].get<double>();
        }
        if (config.saStartTemperature < 0) {
            throw std::invalid_argument(
                "Invalid sa start temperature: " + std::to_string(config.saStartTemperature)
            );
        }
        if (sa.contains("cooling_rate")) {
            config.saCoolingRate = sa["cooling_rate"].get<double>();
        }
        if (config.saCoolingRate < 0 || config.saCoolingRate > 1) {
            throw std::invalid_argument(
                "Invalid sa cooling rate: " + std::to_string(config.saCoolingRate)
            );
        }
        if (sa.contains("debug")) config.saDebug = sa["debug"].get<bool>();
    }

    static void apply_ga_options(DeckRecommendConfig& config, const json_view& opts) {
        if (!opts.contains("ga_options") || !opts["ga_options"].is_object()) {
            return;
        }
        const auto ga = opts["ga_options"];
        if (ga.contains("seed")) config.gaSeed = ga["seed"].get<int>();
        if (ga.contains("debug")) config.gaDebug = ga["debug"].get<bool>();
        if (ga.contains("max_iter")) config.gaMaxIter = ga["max_iter"].get<int>();
        if (ga.contains("max_no_improve_iter")) {
            config.gaMaxIterNoImprove = ga["max_no_improve_iter"].get<int>();
        }
        if (ga.contains("pop_size")) config.gaPopSize = ga["pop_size"].get<int>();
        if (ga.contains("parent_size")) config.gaParentSize = ga["parent_size"].get<int>();
        if (ga.contains("elite_size")) config.gaEliteSize = ga["elite_size"].get<int>();
        if (ga.contains("crossover_rate")) {
            config.gaCrossoverRate = ga["crossover_rate"].get<double>();
        }
        if (ga.contains("base_mutation_rate")) {
            config.gaBaseMutationRate = ga["base_mutation_rate"].get<double>();
        }
        if (ga.contains("no_improve_iter_to_mutation_rate")) {
            config.gaNoImproveIterToMutationRate =
                ga["no_improve_iter_to_mutation_rate"].get<double>();
        }
    }

    static void apply_timeout_option(
        DeckRecommendConfig& config,
        const json_view& opts,
        int default_timeout_ms
    ) {
        if (auto timeout_ms = json_opt<int>(opts, "timeout_ms")) {
            config.timeout_ms = *timeout_ms;
        } else if (default_timeout_ms > 0) {
            config.timeout_ms = default_timeout_ms;
        }
        config.timeout_ms = std::clamp(config.timeout_ms, 1, kMaxRecommendTimeoutMs);
    }

    static DeckRecommendConfig build_recommend_config(
        const json_view& opts,
        const DataProvider& dp,
        const LiveContext& live,
        int default_timeout_ms
    ) {
        DeckRecommendConfig config{};
        apply_target_options(config, opts, live.is_mysekai);
        apply_custom_bonus_options(config, opts);
        apply_algorithm_options(config, opts, live.is_challenge);
        config.filterOtherUnit = json_opt<bool>(opts, "filter_other_unit").value_or(false);
        apply_music_and_size_options(config, opts, dp);
        apply_fixed_card_options(config, opts, dp);
        apply_fixed_character_options(config, opts, live.is_challenge);
        apply_forced_leader_option(config, opts);
        apply_skill_strategy_options(config, opts);
        config.keepAfterTrainingState = json_opt<bool>(opts, "keep_after_training_state")
            .value_or(false);
        apply_multi_live_options(config, opts, live.type);
        config.bestSkillAsLeader = json_opt<bool>(opts, "best_skill_as_leader").value_or(true);
        apply_timeout_option(config, opts, default_timeout_ms);
        apply_card_options(config, opts);
        apply_sa_options(config, opts);
        apply_ga_options(config, opts);
        return config;
    }

    static std::vector<RecommendDeck> execute_recommendation(
        DataProvider& dp,
        int event_id,
        int challenge_character_id,
        int world_bloom_character_id,
        const LiveContext& live,
        const DeckRecommendConfig& config
    ) {
        if (config.target == RecommendTarget::Mysekai) {
            MysekaiDeckRecommend recommender(dp);
            return recommender.recommendMysekaiDeck(
                event_id,
                config,
                world_bloom_character_id
            );
        }
        if (live.is_challenge) {
            ChallengeLiveDeckRecommend recommender(dp);
            return recommender.recommendChallengeLiveDeck(
                live.type,
                challenge_character_id,
                config
            );
        }
        EventDeckRecommend recommender(dp);
        return recommender.recommendEventDeck(
            event_id,
            live.type,
            config,
            world_bloom_character_id
        );
    }

    static std::string serialize_recommendation_result(
        const std::vector<RecommendDeck>& decks,
        double cost_ms
    ) {
        MutableJsonDoc out_doc;
        yyjson_mut_val* result_json = json_object(out_doc.get());
        yyjson_mut_val* decks_json = json_array(out_doc.get());
        for (const auto& deck : decks) {
            json_array_append(decks_json, recommend_deck_to_json(out_doc.get(), deck));
        }
        json_add_value(out_doc.get(), result_json, "decks", decks_json);
        json_add(out_doc.get(), result_json, "cost_ms", cost_ms);
        return dump_mutable_json(result_json);
    }

    static int resolve_support_event_id(const json_view& opts, const MasterData& masterdata) {
        if (auto event_id = json_opt<int>(opts, "event_id")) {
            return *event_id;
        }
        auto turn = json_opt<int>(opts, "world_bloom_event_turn");
        if (!turn.has_value()) {
            throw std::invalid_argument("event_id or world_bloom_event_turn is required.");
        }
        if (*turn < 1 || *turn > 3) {
            throw std::invalid_argument(
                "Invalid world bloom event turn: " + std::to_string(*turn)
            );
        }
        if (*turn == 3) {
            auto character_id = json_opt<int>(opts, "world_bloom_character_id");
            if (!character_id.has_value()) {
                throw std::invalid_argument(
                    "world_bloom_character_id is required for world bloom 3 fake event."
                );
            }
            int part = masterdata.getWorldBloom3PartByCharacterId(*character_id);
            return masterdata.getWorldBloomFakeEventId(*turn, part);
        }
        auto event_unit = json_opt<std::string>(opts, "event_unit");
        if (!event_unit.has_value()) {
            throw std::invalid_argument("event_unit is required for world bloom fake event.");
        }
        if (!VALID_UNIT_TYPES.count(*event_unit)) {
            throw std::invalid_argument("Invalid event unit: " + *event_unit);
        }
        return masterdata.getWorldBloomFakeEventId(
            *turn,
            mapEnum(EnumMap::unit, *event_unit)
        );
    }

    static int resolve_support_character_id(const json_view& opts) {
        auto character_id = json_opt<int>(opts, "world_bloom_character_id");
        if (!character_id.has_value()) {
            character_id = json_opt<int>(opts, "forced_leader_character_id");
        }
        if (!character_id.has_value()) {
            character_id = json_opt<int>(opts, "forcedLeaderCharacterId");
        }
        if (!character_id.has_value()) {
            throw std::invalid_argument(
                "world_bloom_character_id or forcedLeaderCharacterId is required."
            );
        }
        if (*character_id < 1 || *character_id > 26) {
            throw std::invalid_argument(
                "Invalid world_bloom_character_id or forcedLeaderCharacterId: "
                + std::to_string(*character_id)
            );
        }
        return *character_id;
    }

public:
    void update_masterdata(const std::string& base_dir, const std::string& region_str) {
        if (!REGION_MAP.count(region_str)) {
            throw std::invalid_argument("Invalid region: " + region_str);
        }
        auto r = REGION_MAP.at(region_str);
        auto next_masterdata = std::make_shared<MasterData>();
        next_masterdata->loadFromFiles(base_dir);
        shared_region_data_store().set_masterdata(r, std::move(next_masterdata));
    }

    void update_masterdata_from_strings(std::map<std::string, std::string>& data, const std::string& region_str) {
        if (!REGION_MAP.count(region_str)) {
            throw std::invalid_argument("Invalid region: " + region_str);
        }
        auto r = REGION_MAP.at(region_str);
        std::map<std::string, std::string> normalized_data{};
        for (const auto& [raw_key, value] : data) {
            auto normalized_key = normalize_masterdata_key(raw_key);
            auto existing = normalized_data.find(normalized_key);
            if (existing == normalized_data.end() || raw_key == normalized_key) {
                normalized_data[normalized_key] = value;
            }
        }
        auto next_masterdata = std::make_shared<MasterData>();
        next_masterdata->loadFromStrings(normalized_data);
        shared_region_data_store().set_masterdata(r, std::move(next_masterdata));
    }

    void update_musicmetas_file(const std::string& file_path, const std::string& region_str) {
        if (!REGION_MAP.count(region_str)) {
            throw std::invalid_argument("Invalid region: " + region_str);
        }
        auto r = REGION_MAP.at(region_str);
        auto next_musicmetas = std::make_shared<MusicMetas>();
        next_musicmetas->loadFromFile(file_path);
        shared_region_data_store().set_musicmetas(r, std::move(next_musicmetas));
    }

    void update_musicmetas_string(const std::string& s, const std::string& region_str) {
        if (!REGION_MAP.count(region_str)) {
            throw std::invalid_argument("Invalid region: " + region_str);
        }
        auto r = REGION_MAP.at(region_str);
        auto next_musicmetas = std::make_shared<MusicMetas>();
        next_musicmetas->loadFromString(s);
        shared_region_data_store().set_musicmetas(r, std::move(next_musicmetas));
    }

    std::string cache_userdata(std::string_view userdata_str) {
        auto userdata = std::make_shared<UserData>();
        json_doc doc;
        try {
            userdata->path.clear();
            doc = parse_json_bytes(userdata_str.data(), userdata_str.size(), "user data string");
        } catch (const std::exception& e) {
            throw std::runtime_error("Failed to load user data from bytes, error: " + std::string(e.what()));
        }
        userdata->loadFromJson(doc.root());
        auto userdata_hash = hash_userdata_payload(userdata_str);
        remember_userdata(userdata_hash, userdata);
        return userdata_hash;
    }

    void attach_cached_userdata(std::string_view userdata_hash_view) {
        if (userdata_hash_view.empty()) {
            throw std::invalid_argument("userdata_hash is required.");
        }
        attach_shared_userdata(std::string(userdata_hash_view));
    }

    std::string calculate(const json_view& opts) {
        auto mode = require_string_field(opts, "mode");
        auto dp = build_data_provider(opts, mode == "live_full");
        auto deckCards = resolve_fixed_deck_cards(dp, opts, mode);
        auto deckDetail = calculate_fixed_deck_detail(dp, deckCards);

        MutableJsonDoc out_doc;
        yyjson_mut_val* result = json_object(out_doc.get());

        if (mode == "deck" || mode == "challenge") {
            json_add(out_doc.get(), result, "totalPower", deckDetail.power.total);
            json_add_value(out_doc.get(), result, "detail", deck_detail_to_json(out_doc.get(), deckDetail));
            return dump_mutable_json(result);
        }

        if (mode != "live_full") {
            throw std::invalid_argument("Invalid calculate mode: " + mode);
        }

        auto musicId = require_int_field(opts, "music_id");
        auto difficulty = require_string_field(opts, "difficulty");
        if (!VALID_MUSIC_DIFFS.count(difficulty)) {
            throw std::invalid_argument("Invalid music difficulty: " + difficulty);
        }

        LiveCalculator liveCalculator(dp);
        auto musicMeta = liveCalculator.getMusicMeta(
            musicId,
            mapEnum(EnumMap::musicDifficulty, difficulty)
        );
        auto liveSkills = parse_live_skills(opts);
        std::optional<std::vector<DeckCardSkillDetail>> skillDetails = std::nullopt;
        if (liveSkills.has_value()) {
            skillDetails = liveCalculator.getSoloLiveSkill(liveSkills.value(), deckDetail.cards);
        }

        auto liveDetail = liveCalculator.getLiveDetailByDeck(
            deckDetail,
            musicMeta,
            mapEnum(EnumMap::liveType, "solo"),
            LiveSkillOrder::best,
            std::nullopt,
            skillDetails
        );
        liveDetail.deck = deckDetail;

        json_add(out_doc.get(), result, "totalPower", deckDetail.power.total);
        json_add(out_doc.get(), result, "liveScore", liveDetail.score);
        json_add_value(out_doc.get(), result, "deckDetail", deck_detail_to_json(out_doc.get(), deckDetail));
        json_add_value(out_doc.get(), result, "liveDetail", live_detail_to_json(out_doc.get(), liveDetail));
        return dump_mutable_json(result);
    }

    std::string recommend(
        const json_view& opts,
        int default_timeout_ms = 0,
        const char* forced_region = nullptr,
        const char* forced_userdata_hash = nullptr,
        size_t forced_region_len = 0,
        size_t forced_userdata_hash_len = 0
    ) {
        std::string region_str = context_value_or_json(
            opts,
            "region",
            forced_region,
            forced_region_len,
            "region"
        );
        Region region = resolve_region(region_str);
        auto userdata = resolve_userdata(opts, forced_userdata_hash, forced_userdata_hash_len);
        auto masterdata = shared_region_data_store().get_masterdata(region);
        if (!masterdata) {
            throw std::invalid_argument("Master data not found for region: " + region_str);
        }
        auto musicmetas = shared_region_data_store().get_musicmetas(region);
        if (!musicmetas) {
            throw std::invalid_argument("Music metas not found for region: " + region_str);
        }
        DataProvider dp{region, masterdata, userdata, musicmetas};
        LiveContext live = resolve_live_context(opts);
        int event_id = resolve_event_id(opts, dp, live.is_challenge);
        int challenge_character_id = resolve_challenge_character_id(opts, live.is_challenge);
        int world_bloom_character_id = resolve_world_bloom_character_id(opts, dp, event_id);
        DeckRecommendConfig config = build_recommend_config(opts, dp, live, default_timeout_ms);

        // --- execute recommendation ---
        // Match the upstream bindings: cost_ms measures only the search
        // algorithm, excluding option/userdata parsing and result conversion.
        auto search_started = std::chrono::steady_clock::now();
        auto result = execute_recommendation(
            dp,
            event_id,
            challenge_character_id,
            world_bloom_character_id,
            live,
            config
        );
        double cost_ms = std::chrono::duration<double, std::milli>(
            std::chrono::steady_clock::now() - search_started
        ).count();
        return serialize_recommendation_result(result, cost_ms);
    }

    std::string recommend_batch(
        const json_view& options_list,
        int default_timeout_ms,
        const char* forced_region,
        const char* forced_userdata_hash,
        size_t forced_region_len,
        size_t forced_userdata_hash_len
    ) {
        if (!options_list.is_array()) {
            throw std::invalid_argument("batch recommend options must be an array.");
        }
        if (!forced_region || forced_region_len == 0) {
            throw std::invalid_argument("region is required for batch recommend.");
        }
        if (!forced_userdata_hash || forced_userdata_hash_len == 0) {
            throw std::invalid_argument("userdata_hash is required for batch recommend.");
        }

        // Resolve the shared userdata before entering the parallel region so every
        // worker only reads the handle-local cache.
        std::string userdata_hash(forced_userdata_hash, forced_userdata_hash_len);
        if (!userdata_cache.count(userdata_hash)) {
            attach_shared_userdata(userdata_hash);
        }

        std::vector<json_view> options;
        options.reserve(options_list.size());
        for (const auto& option : options_list) {
            if (!option.is_object()) {
                throw std::invalid_argument("batch recommend options entries must be objects.");
            }
            options.push_back(option);
        }

        struct BatchItem {
            std::string result;
            std::string error;
            double cost_time = 0.0;
        };
        std::vector<BatchItem> items(options.size());

        parallelFor(options.size(), [&](std::size_t index) {
            auto started = std::chrono::steady_clock::now();
            try {
                items[index].result = recommend(
                    options[index],
                    default_timeout_ms,
                    forced_region,
                    forced_userdata_hash,
                    forced_region_len,
                    forced_userdata_hash_len
                );
            } catch (const std::exception& e) {
                items[index].error = e.what();
            } catch (...) {
                items[index].error = "Unknown error during batch recommendation";
            }
            items[index].cost_time = std::chrono::duration<double>(
                std::chrono::steady_clock::now() - started
            ).count();
        }, 1);

        MutableJsonDoc out_doc;
        yyjson_mut_val* result_array = json_array(out_doc.get());
        for (const auto& item : items) {
            yyjson_mut_val* item_json = json_object(out_doc.get());
            json_add(out_doc.get(), item_json, "cost_time", item.cost_time);
            if (!item.result.empty()) {
                json_add(out_doc.get(), item_json, "result", item.result);
            }
            if (!item.error.empty()) {
                json_add(out_doc.get(), item_json, "error", item.error);
            }
            json_array_append(result_array, item_json);
        }
        return dump_mutable_json(result_array);
    }

    std::string get_world_bloom_support_cards(const json_view& opts) {
        std::string region_str = require_string_field(opts, "region");
        Region region = resolve_region(region_str);

        auto masterdata = shared_region_data_store().get_masterdata(region);
        if (!masterdata) {
            throw std::invalid_argument("Master data not found for region: " + region_str);
        }

        auto userdata = resolve_userdata(opts);
        int event_id = resolve_support_event_id(opts, *masterdata);
        int character_id = resolve_support_character_id(opts);

        auto musicmetas = shared_region_data_store().get_musicmetas(region);
        if (!musicmetas) {
            musicmetas = std::make_shared<MusicMetas>();
        }
        DataProvider dataProvider{region, masterdata, userdata, musicmetas};
        dataProvider.init();

        bool supportMasterMax = json_opt<bool>(opts, "support_master_max").value_or(false);
        bool supportSkillMax = json_opt<bool>(opts, "support_skill_max").value_or(false);
        bool filterOtherUnit = json_opt<bool>(opts, "filter_other_unit").value_or(false);

        CardCalculator cardCalculator(dataProvider);
        std::vector<std::pair<int, double>> result;
        result.reserve(userdata->userCards.size());
        for (const auto& card : userdata->userCards) {
            auto support_card = cardCalculator.getSupportDeckCard(
                card,
                event_id,
                character_id,
                supportMasterMax,
                supportSkillMax,
                !filterOtherUnit
            );
            result.emplace_back(support_card.cardId, support_card.bonus);
        }
        std::sort(result.begin(), result.end(), [](const auto& a, const auto& b) {
            return std::tuple(a.second, -a.first) > std::tuple(b.second, -b.first);
        });

        MutableJsonDoc out_doc;
        yyjson_mut_val* out = json_array(out_doc.get());
        for (const auto& [card_id, bonus] : result) {
            yyjson_mut_val* card = json_object(out_doc.get());
            json_add(out_doc.get(), card, "card_id", card_id);
            json_add(out_doc.get(), card, "bonus", bonus);
            json_array_append(out, card);
        }
        return dump_mutable_json(out);
    }
};

// ---- C API implementation ----

struct DeckRecommendOpaque {
    SekaiDeckRecommendC engine;
};

static SekaiDeckRecommendC& engine_from_handle(DeckRecommendHandle handle) {
    if (!handle) {
        throw std::invalid_argument("deck recommend handle is required.");
    }
    return handle->engine;
}

extern "C" {

const char* deck_recommend_init_data_path(const char* path) {
    try {
        setStaticDataDir(std::string(path));
        return nullptr;
    } catch (const std::exception& e) {
        return alloc_error(e.what());
    }
}

DeckRecommendHandle deck_recommend_create(void) {
    try {
        return std::make_unique<DeckRecommendOpaque>().release();
    } catch (...) {
        return nullptr;
    }
}

void deck_recommend_destroy(DeckRecommendHandle handle) {
    std::unique_ptr<DeckRecommendOpaque> owned_handle(handle);
}

const char* deck_recommend_update_masterdata(DeckRecommendHandle handle, const char* base_dir, const char* region) {
    try {
        engine_from_handle(handle).update_masterdata(base_dir, region);
        return nullptr;
    } catch (const std::exception& e) {
        return alloc_error(e.what());
    }
}

const char* deck_recommend_update_masterdata_from_json(DeckRecommendHandle handle, const char* json_map, const char* region) {
    return deck_recommend_update_masterdata_from_json_n(
        handle,
        json_map,
        nullable_cstr_size(json_map),
        region
    );
}

const char* deck_recommend_update_masterdata_from_json_n(
    DeckRecommendHandle handle,
    const char* json_map,
    size_t json_map_len,
    const char* region
) {
    try {
        auto doc = parse_json_bytes(json_map, json_map_len, "masterdata map");
        json_view root = doc.root();
        if (!root.is_object()) {
            throw std::invalid_argument("masterdata JSON payload must be an object.");
        }

        std::map<std::string, std::string> data;
        yyjson_obj_iter iter = yyjson_obj_iter_with(root.raw());
        yyjson_val* key = nullptr;
        while ((key = yyjson_obj_iter_next(&iter))) {
            const char* raw_key = yyjson_get_str(key);
            if (!raw_key) {
                continue;
            }
            json_view value(yyjson_obj_iter_get_val(key));
            data[raw_key] = value.is_string() ? value.get<std::string>() : dump_json(value);
        }
        engine_from_handle(handle).update_masterdata_from_strings(data, region);
        return nullptr;
    } catch (const std::exception& e) {
        return alloc_error(e.what());
    }
}

const char* deck_recommend_update_musicmetas(DeckRecommendHandle handle, const char* file_path, const char* region) {
    try {
        engine_from_handle(handle).update_musicmetas_file(file_path, region);
        return nullptr;
    } catch (const std::exception& e) {
        return alloc_error(e.what());
    }
}

const char* deck_recommend_update_musicmetas_from_string(DeckRecommendHandle handle, const char* json_str, const char* region) {
    return deck_recommend_update_musicmetas_from_string_n(
        handle,
        json_str,
        nullable_cstr_size(json_str),
        region
    );
}

const char* deck_recommend_update_musicmetas_from_string_n(
    DeckRecommendHandle handle,
    const char* json_str,
    size_t json_str_len,
    const char* region
) {
    try {
        if (!region || !REGION_MAP.count(region)) {
            throw std::invalid_argument("Invalid region: " + std::string(region ? region : ""));
        }
        auto r = REGION_MAP.at(region);
        auto next_musicmetas = std::make_shared<MusicMetas>();
        json_doc doc;
        try {
            next_musicmetas->path.clear();
            doc = parse_json_bytes(json_str, json_str_len, "music metas string");
        } catch (const std::exception& e) {
            throw std::runtime_error("Failed to load music metas from string, error: " + std::string(e.what()));
        }
        next_musicmetas->loadFromJson(doc.root());
        shared_region_data_store().set_musicmetas(r, std::move(next_musicmetas));
        return nullptr;
    } catch (const std::exception& e) {
        return alloc_error(e.what());
    }
}

const char* deck_recommend_cache_userdata(DeckRecommendHandle handle, const char* userdata_json, const char** hash_out) {
    return deck_recommend_cache_userdata_n(
        handle,
        userdata_json,
        nullable_cstr_size(userdata_json),
        hash_out,
        nullptr
    );
}

const char* deck_recommend_cache_userdata_n(
    DeckRecommendHandle handle,
    const char* userdata_json,
    size_t userdata_json_len,
    const char** hash_out,
    size_t* hash_len_out
) {
    try {
        if (!userdata_json || userdata_json_len == 0) {
            throw std::invalid_argument("userdata_json is required.");
        }
        auto userdata_hash = engine_from_handle(handle).cache_userdata(
            std::string_view(userdata_json, userdata_json_len)
        );
        if (hash_out) {
            *hash_out = alloc_cstr(userdata_hash, hash_len_out);
        } else if (hash_len_out) {
            *hash_len_out = userdata_hash.size();
        }
        return nullptr;
    } catch (const std::exception& e) {
        return alloc_error(e.what());
    }
}

const char* deck_recommend_attach_cached_userdata(DeckRecommendHandle handle, const char* userdata_hash) {
    return deck_recommend_attach_cached_userdata_n(
        handle,
        userdata_hash,
        nullable_cstr_size(userdata_hash)
    );
}

const char* deck_recommend_attach_cached_userdata_n(
    DeckRecommendHandle handle,
    const char* userdata_hash,
    size_t userdata_hash_len
) {
    try {
        if (!userdata_hash || userdata_hash_len == 0) {
            throw std::invalid_argument("userdata_hash is required.");
        }
        engine_from_handle(handle).attach_cached_userdata(
            std::string_view(userdata_hash, userdata_hash_len)
        );
        return nullptr;
    } catch (const std::exception& e) {
        return alloc_error(e.what());
    }
}

const char* deck_recommend_recommend(DeckRecommendHandle handle, const char* options_json, const char** error_out) {
    return deck_recommend_recommend_n(
        handle,
        options_json,
        nullable_cstr_size(options_json),
        error_out,
        nullptr
    );
}

const char* deck_recommend_recommend_n(
    DeckRecommendHandle handle,
    const char* options_json,
    size_t options_json_len,
    const char** error_out,
    size_t* result_len_out
) {
    try {
        auto doc = parse_json_bytes(options_json, options_json_len, "recommend options");
        auto result = engine_from_handle(handle).recommend(doc.root());
        return alloc_cstr(result, result_len_out);
    } catch (const std::exception& e) {
        if (error_out) {
            *error_out = alloc_error(e.what());
        }
        return nullptr;
    }
}

const char* deck_recommend_recommend_with_default_timeout(
    DeckRecommendHandle handle,
    const char* options_json,
    int default_timeout_ms,
    const char** error_out
) {
    return deck_recommend_recommend_with_default_timeout_n(
        handle,
        options_json,
        nullable_cstr_size(options_json),
        default_timeout_ms,
        error_out,
        nullptr
    );
}

const char* deck_recommend_recommend_with_default_timeout_n(
    DeckRecommendHandle handle,
    const char* options_json,
    size_t options_json_len,
    int default_timeout_ms,
    const char** error_out,
    size_t* result_len_out
) {
    try {
        auto doc = parse_json_bytes(options_json, options_json_len, "recommend options");
        auto result = engine_from_handle(handle).recommend(doc.root(), default_timeout_ms);
        return alloc_cstr(result, result_len_out);
    } catch (const std::exception& e) {
        if (error_out) {
            *error_out = alloc_error(e.what());
        }
        return nullptr;
    }
}

const char* deck_recommend_recommend_with_context(
    DeckRecommendHandle handle,
    const char* options_json,
    const char* forced_region,
    const char* forced_userdata_hash,
    int default_timeout_ms,
    const char** error_out
) {
    return deck_recommend_recommend_with_context_n(
        handle,
        options_json,
        nullable_cstr_size(options_json),
        forced_region,
        nullable_cstr_size(forced_region),
        forced_userdata_hash,
        nullable_cstr_size(forced_userdata_hash),
        default_timeout_ms,
        error_out,
        nullptr
    );
}

const char* deck_recommend_recommend_with_context_n(
    DeckRecommendHandle handle,
    const char* options_json,
    size_t options_json_len,
    const char* forced_region,
    size_t forced_region_len,
    const char* forced_userdata_hash,
    size_t forced_userdata_hash_len,
    int default_timeout_ms,
    const char** error_out,
    size_t* result_len_out
) {
    try {
        if (forced_region_len == 0) {
            forced_region = nullptr;
        }
        if (forced_userdata_hash_len == 0) {
            forced_userdata_hash = nullptr;
        }
        auto doc = parse_json_bytes(options_json, options_json_len, "recommend options");
        auto result = engine_from_handle(handle).recommend(
            doc.root(),
            default_timeout_ms,
            forced_region,
            forced_userdata_hash,
            forced_region_len,
            forced_userdata_hash_len
        );
        return alloc_cstr(result, result_len_out);
    } catch (const std::exception& e) {
        if (error_out) {
            *error_out = alloc_error(e.what());
        }
        return nullptr;
    }
}

const char* deck_recommend_recommend_batch_with_context_n(
    DeckRecommendHandle handle,
    const char* options_json,
    size_t options_json_len,
    const char* forced_region,
    size_t forced_region_len,
    const char* forced_userdata_hash,
    size_t forced_userdata_hash_len,
    int default_timeout_ms,
    const char** error_out,
    size_t* result_len_out
) {
    try {
        auto doc = parse_json_bytes(options_json, options_json_len, "batch recommend options");
        auto result = engine_from_handle(handle).recommend_batch(
            doc.root(),
            default_timeout_ms,
            forced_region,
            forced_userdata_hash,
            forced_region_len,
            forced_userdata_hash_len
        );
        return alloc_cstr(result, result_len_out);
    } catch (const std::exception& e) {
        if (error_out) {
            *error_out = alloc_error(e.what());
        }
        return nullptr;
    }
}

const char* deck_recommend_calculate(DeckRecommendHandle handle, const char* options_json, const char** error_out) {
    return deck_recommend_calculate_n(
        handle,
        options_json,
        nullable_cstr_size(options_json),
        error_out,
        nullptr
    );
}

const char* deck_recommend_calculate_n(
    DeckRecommendHandle handle,
    const char* options_json,
    size_t options_json_len,
    const char** error_out,
    size_t* result_len_out
) {
    try {
        auto doc = parse_json_bytes(options_json, options_json_len, "calculate options");
        auto result = engine_from_handle(handle).calculate(doc.root());
        return alloc_cstr(result, result_len_out);
    } catch (const std::exception& e) {
        if (error_out) {
            *error_out = alloc_error(e.what());
        }
        return nullptr;
    }
}

const char* deck_recommend_get_world_bloom_support_cards(DeckRecommendHandle handle, const char* options_json, const char** error_out) {
    return deck_recommend_get_world_bloom_support_cards_n(
        handle,
        options_json,
        nullable_cstr_size(options_json),
        error_out,
        nullptr
    );
}

const char* deck_recommend_get_world_bloom_support_cards_n(
    DeckRecommendHandle handle,
    const char* options_json,
    size_t options_json_len,
    const char** error_out,
    size_t* result_len_out
) {
    try {
        auto doc = parse_json_bytes(options_json, options_json_len, "world bloom support options");
        auto result = engine_from_handle(handle).get_world_bloom_support_cards(doc.root());
        return alloc_cstr(result, result_len_out);
    } catch (const std::exception& e) {
        if (error_out) {
            *error_out = alloc_error(e.what());
        }
        return nullptr;
    }
}

void deck_recommend_free_string(const char* str) {
    std::unique_ptr<const char[]> owned_string(str);
}

} // extern "C"
