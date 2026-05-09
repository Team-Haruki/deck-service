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

#include <nlohmann/json.hpp>
#include <algorithm>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <deque>
#include <map>
#include <memory>
#include <set>
#include <shared_mutex>
#include <stdexcept>
#include <string>
#include <unordered_map>

using json = nlohmann::json;

namespace {
constexpr int kMaxRecommendTimeoutMs = 5000;
}

// ---- helpers ----

static char* alloc_cstr(const std::string& s) {
    char* p = (char*)malloc(s.size() + 1);
    if (p) {
        memcpy(p, s.c_str(), s.size() + 1);
    }
    return p;
}

static char* alloc_error(const std::string& msg) {
    return alloc_cstr(msg);
}

static std::string hash_userdata_payload(const std::string& payload) {
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

static std::string require_string_field(const json& opts, const std::string& key) {
    if (!opts.contains(key) || !opts[key].is_string())
        throw std::invalid_argument(key + " is required.");
    return opts[key].get<std::string>();
}

static int require_int_field(const json& opts, const std::string& key) {
    if (!opts.contains(key) || !opts[key].is_number_integer())
        throw std::invalid_argument(key + " is required.");
    return opts[key].get<int>();
}

static bool has_int_field(const json& opts, const std::string& key) {
    return opts.contains(key) && !opts[key].is_null() && opts[key].is_number_integer();
}

static json deck_power_to_json(const DeckPowerDetail& power) {
    json j;
    j["base"] = power.base;
    j["areaItemBonus"] = power.areaItemBonus;
    j["characterBonus"] = power.characterBonus;
    j["honorBonus"] = power.honorBonus;
    j["fixtureBonus"] = power.fixtureBonus;
    j["gateBonus"] = power.gateBonus;
    j["total"] = power.total;
    return j;
}

static json deck_card_power_to_json(const DeckCardPowerDetail& power) {
    json j;
    j["base"] = power.base;
    j["areaItemBonus"] = power.areaItemBonus;
    j["characterBonus"] = power.characterBonus;
    j["fixtureBonus"] = power.fixtureBonus;
    j["gateBonus"] = power.gateBonus;
    j["total"] = power.total;
    return j;
}

static json deck_card_skill_to_json(const DeckCardSkillDetail& skill) {
    json j;
    j["scoreUp"] = skill.scoreUp;
    j["lifeRecovery"] = skill.lifeRecovery;
    return j;
}

static json deck_detail_to_json(const DeckDetail& deckDetail) {
    json j;
    j["power"] = deck_power_to_json(deckDetail.power);
    if (deckDetail.eventBonus.has_value())
        j["eventBonus"] = deckDetail.eventBonus.value();
    if (deckDetail.supportDeckBonus.has_value())
        j["supportDeckBonus"] = deckDetail.supportDeckBonus.value();

    json cards = json::array();
    for (const auto& card : deckDetail.cards) {
        json cj;
        cj["cardId"] = card.cardId;
        cj["level"] = card.level;
        cj["skillLevel"] = card.skillLevel;
        cj["masterRank"] = card.masterRank;
        cj["power"] = deck_card_power_to_json(card.power);
        cj["skill"] = deck_card_skill_to_json(card.skill);
        cards.push_back(cj);
    }
    j["cards"] = cards;
    return j;
}

static json live_detail_to_json(const LiveDetail& liveDetail) {
    json j;
    j["score"] = liveDetail.score;
    j["time"] = liveDetail.time;
    j["life"] = liveDetail.life;
    j["tap"] = liveDetail.tap;
    if (liveDetail.deck.has_value())
        j["deck"] = deck_detail_to_json(liveDetail.deck.value());
    return j;
}

static std::optional<std::vector<LiveSkill>> parse_live_skills(const json& opts) {
    if (!opts.contains("skills") || opts["skills"].is_null())
        return std::nullopt;
    if (!opts["skills"].is_array())
        throw std::invalid_argument("skills must be an array.");

    std::vector<LiveSkill> liveSkills{};
    for (const auto& item : opts["skills"]) {
        if (!item.is_object())
            throw std::invalid_argument("skills entries must be objects.");
        LiveSkill liveSkill{};
        if (item.contains("seq") && item["seq"].is_number_integer())
            liveSkill.seq = item["seq"].get<int>();
        if (item.contains("cardId") && item["cardId"].is_number_integer())
            liveSkill.cardId = item["cardId"].get<int>();
        else if (item.contains("card_id") && item["card_id"].is_number_integer())
            liveSkill.cardId = item["card_id"].get<int>();
        else
            throw std::invalid_argument("skills entries require cardId.");
        liveSkills.push_back(liveSkill);
    }
    return liveSkills;
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
// per region across every SekaiDeckRecommendC handle avoids paying N×|data|
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
}

// ---- internal SekaiDeckRecommend wrapper (same logic as pybind11 version) ----

class SekaiDeckRecommendC {
    std::unordered_map<std::string, std::shared_ptr<UserData>> userdata_cache;
    std::deque<std::string> userdata_cache_order;

    static constexpr std::size_t max_userdata_cache_entries = 64;

    void remember_userdata(
        const std::string& userdata_hash,
        const std::shared_ptr<UserData>& userdata
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
    }

    std::shared_ptr<UserData> resolve_userdata(const json& opts) {
        if (opts.contains("userdata_hash") && !opts["userdata_hash"].is_null()) {
            if (!opts["userdata_hash"].is_string())
                throw std::invalid_argument("userdata_hash must be a string.");

            std::string userdata_hash = opts["userdata_hash"];
            auto it = userdata_cache.find(userdata_hash);
            if (it == userdata_cache.end())
                throw std::invalid_argument("User data not found for userdata_hash: " + userdata_hash);
            return it->second;
        }

        auto userdata = std::make_shared<UserData>();
        if (opts.contains("user_data_file_path") && opts["user_data_file_path"].is_string()) {
            userdata->loadFromFile(opts["user_data_file_path"].get<std::string>());
            return userdata;
        }

        if (opts.contains("user_data_str") && opts["user_data_str"].is_string()) {
            auto userdata_str = opts["user_data_str"].get<std::string>();
            userdata->loadFromString(userdata_str);
            remember_userdata(hash_userdata_payload(userdata_str), userdata);
            return userdata;
        }

        throw std::invalid_argument(
            "Either userdata_hash, user_data_file_path or user_data_str is required."
        );
    }

    DataProvider build_data_provider(const json& opts, bool require_musicmetas) {
        if (!opts.contains("region") || !opts["region"].is_string())
            throw std::invalid_argument("region is required.");
        std::string region_str = opts["region"];
        if (!REGION_MAP.count(region_str))
            throw std::invalid_argument("Invalid region: " + region_str);
        Region region = REGION_MAP.at(region_str);

        auto userdata = resolve_userdata(opts);

        auto masterdata = shared_region_data_store().get_masterdata(region);
        if (!masterdata)
            throw std::invalid_argument("Master data not found for region: " + region_str);
        auto musicmetas = shared_region_data_store().get_musicmetas(region);
        if (require_musicmetas && !musicmetas)
            throw std::invalid_argument("Music metas not found for region: " + region_str);
        if (!musicmetas)
            musicmetas = std::make_shared<MusicMetas>();
        return DataProvider{region, masterdata, userdata, musicmetas};
    }

    std::vector<UserCard> resolve_fixed_deck_cards(DataProvider& dp, const json& opts, const std::string& mode) {
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
        if (deckCards.empty())
            throw std::invalid_argument("fixed deck contains no cards.");

        dp.init();

        CardCalculator cardCalculator(dp);
        DeckCalculator deckCalculator(dp);
        std::unordered_map<int, CardConfig> config{};
        std::unordered_map<int, CardConfig> singleCardConfig{};
        auto cardDetails = cardCalculator.batchGetCardDetail(deckCards, config, singleCardConfig);
        if (cardDetails.size() != deckCards.size())
            throw std::runtime_error("Failed to calculate all cards in fixed deck.");

        std::vector<const CardDetail*> cardDetailPtrs{};
        cardDetailPtrs.reserve(cardDetails.size());
        for (const auto& cardDetail : cardDetails)
            cardDetailPtrs.push_back(&cardDetail);

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
        if (deckDetails.empty())
            throw std::runtime_error("Failed to calculate fixed deck detail.");
        return deckDetails.front();
    }

public:
    void update_masterdata(const std::string& base_dir, const std::string& region_str) {
        if (!REGION_MAP.count(region_str))
            throw std::invalid_argument("Invalid region: " + region_str);
        auto r = REGION_MAP.at(region_str);
        auto next_masterdata = std::make_shared<MasterData>();
        next_masterdata->loadFromFiles(base_dir);
        shared_region_data_store().set_masterdata(r, std::move(next_masterdata));
    }

    void update_masterdata_from_strings(std::map<std::string, std::string>& data, const std::string& region_str) {
        if (!REGION_MAP.count(region_str))
            throw std::invalid_argument("Invalid region: " + region_str);
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
        if (!REGION_MAP.count(region_str))
            throw std::invalid_argument("Invalid region: " + region_str);
        auto r = REGION_MAP.at(region_str);
        auto next_musicmetas = std::make_shared<MusicMetas>();
        next_musicmetas->loadFromFile(file_path);
        shared_region_data_store().set_musicmetas(r, std::move(next_musicmetas));
    }

    void update_musicmetas_string(const std::string& s, const std::string& region_str) {
        if (!REGION_MAP.count(region_str))
            throw std::invalid_argument("Invalid region: " + region_str);
        auto r = REGION_MAP.at(region_str);
        auto next_musicmetas = std::make_shared<MusicMetas>();
        next_musicmetas->loadFromString(s);
        shared_region_data_store().set_musicmetas(r, std::move(next_musicmetas));
    }

    std::string cache_userdata(const std::string& userdata_str) {
        auto userdata = std::make_shared<UserData>();
        userdata->loadFromString(userdata_str);
        auto userdata_hash = hash_userdata_payload(userdata_str);
        remember_userdata(userdata_hash, userdata);
        return userdata_hash;
    }

    json calculate(const json& opts) {
        auto mode = require_string_field(opts, "mode");
        auto dp = build_data_provider(opts, mode == "live_full");
        auto deckCards = resolve_fixed_deck_cards(dp, opts, mode);
        auto deckDetail = calculate_fixed_deck_detail(dp, deckCards);

        if (mode == "deck" || mode == "challenge") {
            json result;
            result["totalPower"] = deckDetail.power.total;
            result["detail"] = deck_detail_to_json(deckDetail);
            return result;
        }

        if (mode != "live_full")
            throw std::invalid_argument("Invalid calculate mode: " + mode);

        auto musicId = require_int_field(opts, "music_id");
        auto difficulty = require_string_field(opts, "difficulty");
        if (!VALID_MUSIC_DIFFS.count(difficulty))
            throw std::invalid_argument("Invalid music difficulty: " + difficulty);

        LiveCalculator liveCalculator(dp);
        auto musicMeta = liveCalculator.getMusicMeta(
            musicId,
            mapEnum(EnumMap::musicDifficulty, difficulty)
        );
        auto liveSkills = parse_live_skills(opts);
        std::optional<std::vector<DeckCardSkillDetail>> skillDetails = std::nullopt;
        if (liveSkills.has_value())
            skillDetails = liveCalculator.getSoloLiveSkill(liveSkills.value(), deckDetail.cards);

        auto liveDetail = liveCalculator.getLiveDetailByDeck(
            deckDetail,
            musicMeta,
            mapEnum(EnumMap::liveType, "solo"),
            LiveSkillOrder::best,
            std::nullopt,
            skillDetails
        );
        liveDetail.deck = deckDetail;

        json result;
        result["totalPower"] = deckDetail.power.total;
        result["liveScore"] = liveDetail.score;
        result["deckDetail"] = deck_detail_to_json(deckDetail);
        result["liveDetail"] = live_detail_to_json(liveDetail);
        return result;
    }

    json recommend(const json& opts) {
        // --- region ---
        if (!opts.contains("region") || !opts["region"].is_string())
            throw std::invalid_argument("region is required.");
        std::string region_str = opts["region"];
        if (!REGION_MAP.count(region_str))
            throw std::invalid_argument("Invalid region: " + region_str);
        Region region = REGION_MAP.at(region_str);

        // --- user data ---
        auto userdata = resolve_userdata(opts);

        // --- master data & music metas ---
        auto masterdata = shared_region_data_store().get_masterdata(region);
        if (!masterdata)
            throw std::invalid_argument("Master data not found for region: " + region_str);
        auto musicmetas = shared_region_data_store().get_musicmetas(region);
        if (!musicmetas)
            throw std::invalid_argument("Music metas not found for region: " + region_str);

        DataProvider dp{region, masterdata, userdata, musicmetas};

        // --- live type ---
        if (!opts.contains("live_type") || !opts["live_type"].is_string())
            throw std::invalid_argument("live_type is required.");
        std::string live_type_str = opts["live_type"];
        if (!VALID_LIVE_TYPES.count(live_type_str))
            throw std::invalid_argument("Invalid live type: " + live_type_str);

        bool is_mysekai = (live_type_str == "mysekai");
        int liveType;
        if (is_mysekai)
            liveType = mapEnum(EnumMap::liveType, "multi");
        else
            liveType = mapEnum(EnumMap::liveType, live_type_str);
        bool is_challenge = Enums::LiveType::isChallenge(liveType);

        // --- event id ---
        int eventId = 0;
        if (opts.contains("event_id") && !opts["event_id"].is_null()) {
            if (is_challenge)
                throw std::invalid_argument("event_id is not valid for challenge live.");
            eventId = opts["event_id"].get<int>();
            findOrThrow(dp.masterData->events, [&](const Event& it) {
                return it.id == eventId;
            }, "Event not found for eventId: " + std::to_string(eventId));
        } else if (!is_challenge) {
            std::string event_type_str = opts.value("event_type", "marathon");
            if (!VALID_EVENT_TYPES.count(event_type_str))
                throw std::invalid_argument("Invalid event type: " + event_type_str);
            auto event_type_enum = mapEnum(EnumMap::eventType, event_type_str);

            if (opts.contains("world_bloom_event_turn") && !opts["world_bloom_event_turn"].is_null()) {
                int turn = opts["world_bloom_event_turn"].get<int>();
                if (turn < 1 || turn > 3)
                    throw std::invalid_argument("Invalid world bloom event turn.");
                if (turn == 3) {
                    if (!opts.contains("world_bloom_character_id") || opts["world_bloom_character_id"].is_null())
                        throw std::invalid_argument("world_bloom_character_id is required for world bloom 3 fake event.");
                    int characterId = opts["world_bloom_character_id"].get<int>();
                    int part = dp.masterData->getWorldBloom3PartByCharacterId(characterId);
                    eventId = dp.masterData->getWorldBloomFakeEventId(turn, part);
                } else {
                    if (!opts.contains("event_unit") || !opts["event_unit"].is_string())
                        throw std::invalid_argument("event_unit is required for world bloom fake event.");
                    std::string eu = opts["event_unit"];
                    if (!VALID_UNIT_TYPES.count(eu))
                        throw std::invalid_argument("Invalid event unit: " + eu);
                    eventId = dp.masterData->getWorldBloomFakeEventId(turn, mapEnum(EnumMap::unit, eu));
                }
            } else if (opts.contains("event_attr") || opts.contains("event_unit")) {
                if (!opts.contains("event_attr") || !opts.contains("event_unit"))
                    throw std::invalid_argument("event_attr and event_unit must be specified together.");
                std::string ea = opts["event_attr"], eu = opts["event_unit"];
                if (!VALID_EVENT_ATTRS.count(ea)) throw std::invalid_argument("Invalid event attr: " + ea);
                if (!VALID_UNIT_TYPES.count(eu)) throw std::invalid_argument("Invalid event unit: " + eu);
                eventId = dp.masterData->getUnitAttrFakeEventId(
                    event_type_enum, mapEnum(EnumMap::unit, eu), mapEnum(EnumMap::attr, ea));
            } else {
                eventId = dp.masterData->getNoEventFakeEventId(event_type_enum);
            }
        }

        // --- challenge character id ---
        int challengeCharId = 0;
        if (opts.contains("challenge_live_character_id") && !opts["challenge_live_character_id"].is_null()) {
            challengeCharId = opts["challenge_live_character_id"].get<int>();
            if (challengeCharId < 1 || challengeCharId > 26)
                throw std::invalid_argument("Invalid challenge character ID.");
        } else if (is_challenge) {
            throw std::invalid_argument("challenge_live_character_id is required for challenge live.");
        }

        // --- world bloom character id ---
        int worldBloomCharId = 0;
        if (opts.contains("world_bloom_character_id") && !opts["world_bloom_character_id"].is_null()) {
            worldBloomCharId = opts["world_bloom_character_id"].get<int>();
            if (worldBloomCharId < 1 || worldBloomCharId > 26)
                throw std::invalid_argument("Invalid world bloom character ID.");
            findOrThrow(dp.masterData->worldBlooms, [&](const WorldBloom& it) {
                return it.eventId == eventId && it.gameCharacterId == worldBloomCharId;
            }, std::string("World bloom chapter not found."));
        }

        // --- config ---
        DeckRecommendConfig config{};

        // target
        if (is_mysekai) {
            config.target = RecommendTarget::Mysekai;
        } else {
            std::string target = opts.value("target", "score");
            if (!VALID_TARGETS.count(target)) throw std::invalid_argument("Invalid target: " + target);
            if (target == "score") config.target = RecommendTarget::Score;
            else if (target == "skill") config.target = RecommendTarget::Skill;
            else if (target == "power") config.target = RecommendTarget::Power;
            else if (target == "bonus") config.target = RecommendTarget::Bonus;
        }

        // bonus list
        if (opts.contains("target_bonus_list") && opts["target_bonus_list"].is_array() && !opts["target_bonus_list"].empty()) {
            if (config.target != RecommendTarget::Bonus)
                throw std::invalid_argument("target_bonus_list is only valid for bonus target.");
            config.bonusList = opts["target_bonus_list"].get<std::vector<int>>();
        } else if (config.target == RecommendTarget::Bonus) {
            throw std::invalid_argument("target_bonus_list is required for bonus target.");
        }

        // algorithm
        std::string algorithm = opts.value("algorithm", "ga");
        if (!VALID_ALGORITHMS.count(algorithm)) throw std::invalid_argument("Invalid algorithm: " + algorithm);
        if (algorithm == "sa") config.algorithm = RecommendAlgorithm::SA;
        else if (algorithm == "dfs") config.algorithm = RecommendAlgorithm::DFS;
        else if (algorithm == "ga") config.algorithm = RecommendAlgorithm::GA;
        else if (algorithm == "dfs_ga" || algorithm == "dfs-ga") config.algorithm = RecommendAlgorithm::DFS_GA;
        else if (algorithm == "rl") config.algorithm = RecommendAlgorithm::RL;

        // filter other unit
        config.filterOtherUnit = opts.value("filter_other_unit", false);

        // music
        if (!opts.contains("music_id")) throw std::invalid_argument("music_id is required.");
        if (!opts.contains("music_diff")) throw std::invalid_argument("music_diff is required.");
        config.musicId = opts["music_id"].get<int>();
        std::string music_diff = opts["music_diff"];
        if (!VALID_MUSIC_DIFFS.count(music_diff)) throw std::invalid_argument("Invalid music difficulty: " + music_diff);
        config.musicDiff = mapEnum(EnumMap::musicDifficulty, music_diff);
        findOrThrow(dp.musicMetas->metas, [&](const MusicMeta& it) {
            return it.music_id == config.musicId && it.difficulty == config.musicDiff;
        }, "Music meta not found for musicId: " + std::to_string(config.musicId));

        // limit, member
        config.limit = opts.value("limit", 10);
        if (config.limit < 1) throw std::invalid_argument("Invalid limit.");
        config.member = opts.value("member", 5);
        if (config.member < 2 || config.member > 5) throw std::invalid_argument("Invalid member count.");

        // fixed cards
        if (opts.contains("fixed_cards") && opts["fixed_cards"].is_array()) {
            config.fixedCards = opts["fixed_cards"].get<std::vector<int>>();
            if ((int)config.fixedCards.size() > config.member)
                throw std::invalid_argument("Fixed cards size exceeds member count.");
            for (auto cid : config.fixedCards) {
                findOrThrow(dp.masterData->cards, [&](const Card& it) { return it.id == cid; },
                    "Invalid fixed card ID: " + std::to_string(cid));
            }
        }

        // fixed characters
        if (opts.contains("fixed_characters") && opts["fixed_characters"].is_array()) {
            config.fixedCharacters = opts["fixed_characters"].get<std::vector<int>>();
            if ((int)config.fixedCharacters.size() > config.member)
                throw std::invalid_argument("Fixed characters size exceeds member count.");
            if (is_challenge)
                throw std::invalid_argument("fixed_characters is not valid for challenge live.");
            for (auto characterId : config.fixedCharacters) {
                if (characterId < 1 || characterId > 26)
                    throw std::invalid_argument("Invalid fixed character ID: " + std::to_string(characterId));
            }
        }

        // final chapter forced leader
        const char* forcedLeaderKey = nullptr;
        if (opts.contains("forced_leader_character_id") && !opts["forced_leader_character_id"].is_null()) {
            forcedLeaderKey = "forced_leader_character_id";
        } else if (opts.contains("forcedLeaderCharacterId") && !opts["forcedLeaderCharacterId"].is_null()) {
            forcedLeaderKey = "forcedLeaderCharacterId";
        }
        if (forcedLeaderKey != nullptr) {
            auto characterId = opts[forcedLeaderKey].get<int>();
            if (characterId < 1 || characterId > 26)
                throw std::invalid_argument("Invalid forced leader character ID: " + std::to_string(characterId));
            config.forcedLeaderCharacterId = characterId;
        }

        // skill reference choose strategy
        {
            std::string s = opts.value("skill_reference_choose_strategy", "average");
            if (!VALID_SKILL_REF_STRATEGIES.count(s)) throw std::invalid_argument("Invalid skill ref strategy: " + s);
            if (s == "average") config.skillReferenceChooseStrategy = SkillReferenceChooseStrategy::Average;
            else if (s == "max") config.skillReferenceChooseStrategy = SkillReferenceChooseStrategy::Max;
            else if (s == "min") config.skillReferenceChooseStrategy = SkillReferenceChooseStrategy::Min;
        }

        // keep after training state
        config.keepAfterTrainingState = opts.value("keep_after_training_state", false);

        // multi live teammate score up
        if (opts.contains("multi_live_teammate_score_up") && !opts["multi_live_teammate_score_up"].is_null()) {
            config.multiTeammateScoreUp = opts["multi_live_teammate_score_up"].get<int>();
            if (!Enums::LiveType::isMulti(liveType))
                throw std::invalid_argument("multi_live_teammate_score_up is only valid for multi live.");
        }

        // multi live teammate power
        if (opts.contains("multi_live_teammate_power") && !opts["multi_live_teammate_power"].is_null()) {
            config.multiTeammatePower = opts["multi_live_teammate_power"].get<int>();
            if (!Enums::LiveType::isMulti(liveType))
                throw std::invalid_argument("multi_live_teammate_power is only valid for multi live.");
        }

        // best skill as leader
        config.bestSkillAsLeader = opts.value("best_skill_as_leader", true);

        // multi live score up lower bound
        if (opts.contains("multi_live_score_up_lower_bound") && !opts["multi_live_score_up_lower_bound"].is_null()) {
            if (!Enums::LiveType::isMulti(liveType))
                throw std::invalid_argument("multi_live_score_up_lower_bound is only valid for multi live.");
            config.multiScoreUpLowerBound = opts["multi_live_score_up_lower_bound"].get<double>();
        }

        // skill order choose strategy
        {
            std::string s = opts.value("skill_order_choose_strategy", "average");
            if (!VALID_SKILL_ORDER_STRATEGIES.count(s)) throw std::invalid_argument("Invalid skill order strategy: " + s);
            if (s == "average") config.liveSkillOrder = LiveSkillOrder::average;
            else if (s == "max") config.liveSkillOrder = LiveSkillOrder::best;
            else if (s == "min") config.liveSkillOrder = LiveSkillOrder::worst;
            else if (s == "specific") config.liveSkillOrder = LiveSkillOrder::specific;
        }

        // specific skill order
        if (opts.contains("specific_skill_order") && opts["specific_skill_order"].is_array()) {
            config.specificSkillOrder = opts["specific_skill_order"].get<std::vector<int>>();
        }

        // timeout
        if (opts.contains("timeout_ms") && !opts["timeout_ms"].is_null()) {
            config.timeout_ms = opts["timeout_ms"].get<int>();
        }
        config.timeout_ms = std::clamp(config.timeout_ms, 1, kMaxRecommendTimeoutMs);

        // card config helper
        auto apply_card_config = [&](const std::string& key, const json& cfg) {
            CardConfig cc{};
            if (cfg.contains("disable")) cc.disable = cfg["disable"].get<bool>();
            if (cfg.contains("level_max")) cc.rankMax = cfg["level_max"].get<bool>();
            if (cfg.contains("episode_read")) cc.episodeRead = cfg["episode_read"].get<bool>();
            if (cfg.contains("master_max")) cc.masterMax = cfg["master_max"].get<bool>();
            if (cfg.contains("skill_max")) cc.skillMax = cfg["skill_max"].get<bool>();
            if (cfg.contains("canvas")) cc.canvas = cfg["canvas"].get<bool>();
            config.cardConfig[mapEnum(EnumMap::cardRarityType, key)] = cc;
        };

        // rarity configs
        for (const auto& rk : {"rarity_1", "rarity_2", "rarity_3", "rarity_birthday", "rarity_4"}) {
            std::string key = std::string(rk) + "_config";
            if (opts.contains(key) && opts[key].is_object()) {
                apply_card_config(rk, opts[key]);
            } else {
                config.cardConfig[mapEnum(EnumMap::cardRarityType, rk)] = CardConfig{};
            }
        }

        // single card configs
        if (opts.contains("single_card_configs") && opts["single_card_configs"].is_array()) {
            for (const auto& item : opts["single_card_configs"]) {
                CardConfig cc{};
                if (item.contains("disable")) cc.disable = item["disable"].get<bool>();
                if (item.contains("level_max")) cc.rankMax = item["level_max"].get<bool>();
                if (item.contains("episode_read")) cc.episodeRead = item["episode_read"].get<bool>();
                if (item.contains("master_max")) cc.masterMax = item["master_max"].get<bool>();
                if (item.contains("skill_max")) cc.skillMax = item["skill_max"].get<bool>();
                if (item.contains("canvas")) cc.canvas = item["canvas"].get<bool>();
                config.singleCardConfig[item["card_id"].get<int>()] = cc;
            }
        }
        if (opts.contains("support_master_max") && !opts["support_master_max"].is_null()) {
            config.supportMasterMax = opts["support_master_max"].get<bool>();
        }
        if (opts.contains("support_skill_max") && !opts["support_skill_max"].is_null()) {
            config.supportSkillMax = opts["support_skill_max"].get<bool>();
        }

        // SA options
        if (opts.contains("sa_options") && opts["sa_options"].is_object()) {
            const auto& sa = opts["sa_options"];
            if (sa.contains("run_num")) config.saRunCount = sa["run_num"].get<int>();
            if (config.saRunCount < 1)
                throw std::invalid_argument("Invalid sa run count: " + std::to_string(config.saRunCount));

            if (sa.contains("seed")) config.saSeed = sa["seed"].get<int>();

            if (sa.contains("max_iter")) config.saMaxIter = sa["max_iter"].get<int>();
            if (config.saMaxIter < 1)
                throw std::invalid_argument("Invalid sa max iter: " + std::to_string(config.saMaxIter));

            if (sa.contains("max_no_improve_iter"))
                config.saMaxIterNoImprove = sa["max_no_improve_iter"].get<int>();
            if (config.saMaxIterNoImprove < 1)
                throw std::invalid_argument("Invalid sa max no improve iter: " + std::to_string(config.saMaxIterNoImprove));

            if (sa.contains("time_limit_ms")) config.saMaxTimeMs = sa["time_limit_ms"].get<int>();
            if (config.saMaxTimeMs < 0)
                throw std::invalid_argument("Invalid sa max time ms: " + std::to_string(config.saMaxTimeMs));

            if (sa.contains("start_temprature"))
                config.saStartTemperature = sa["start_temprature"].get<double>();
            else if (sa.contains("start_temperature"))
                config.saStartTemperature = sa["start_temperature"].get<double>();
            if (config.saStartTemperature < 0)
                throw std::invalid_argument("Invalid sa start temperature: " + std::to_string(config.saStartTemperature));

            if (sa.contains("cooling_rate")) config.saCoolingRate = sa["cooling_rate"].get<double>();
            if (config.saCoolingRate < 0 || config.saCoolingRate > 1)
                throw std::invalid_argument("Invalid sa cooling rate: " + std::to_string(config.saCoolingRate));

            if (sa.contains("debug")) config.saDebug = sa["debug"].get<bool>();
        }

        // GA options
        if (opts.contains("ga_options") && opts["ga_options"].is_object()) {
            const auto& ga = opts["ga_options"];
            if (ga.contains("seed")) config.gaSeed = ga["seed"].get<int>();
            if (ga.contains("debug")) config.gaDebug = ga["debug"].get<bool>();
            if (ga.contains("max_iter")) config.gaMaxIter = ga["max_iter"].get<int>();
            if (ga.contains("max_no_improve_iter")) config.gaMaxIterNoImprove = ga["max_no_improve_iter"].get<int>();
            if (ga.contains("pop_size")) config.gaPopSize = ga["pop_size"].get<int>();
            if (ga.contains("parent_size")) config.gaParentSize = ga["parent_size"].get<int>();
            if (ga.contains("elite_size")) config.gaEliteSize = ga["elite_size"].get<int>();
            if (ga.contains("crossover_rate")) config.gaCrossoverRate = ga["crossover_rate"].get<double>();
            if (ga.contains("base_mutation_rate")) config.gaBaseMutationRate = ga["base_mutation_rate"].get<double>();
            if (ga.contains("no_improve_iter_to_mutation_rate"))
                config.gaNoImproveIterToMutationRate = ga["no_improve_iter_to_mutation_rate"].get<double>();
        }

        // --- execute recommendation ---
        std::vector<RecommendDeck> result;

        if (config.target == RecommendTarget::Mysekai) {
            MysekaiDeckRecommend rec(dp);
            result = rec.recommendMysekaiDeck(eventId, config, worldBloomCharId);
        } else if (Enums::LiveType::isChallenge(liveType)) {
            ChallengeLiveDeckRecommend rec(dp);
            result = rec.recommendChallengeLiveDeck(liveType, challengeCharId, config);
        } else {
            EventDeckRecommend rec(dp);
            result = rec.recommendEventDeck(eventId, liveType, config, worldBloomCharId);
        }

        // --- build response JSON ---
        json decks_json = json::array();
        for (const auto& deck : result) {
            json dj;
            dj["score"] = deck.score;
            dj["live_score"] = deck.liveScore;
            dj["mysekai_event_point"] = deck.mysekaiEventPoint;
            dj["total_power"] = deck.power.total;
            dj["base_power"] = deck.power.base;
            dj["area_item_bonus_power"] = deck.power.areaItemBonus;
            dj["character_bonus_power"] = deck.power.characterBonus;
            dj["honor_bonus_power"] = deck.power.honorBonus;
            dj["fixture_bonus_power"] = deck.power.fixtureBonus;
            dj["gate_bonus_power"] = deck.power.gateBonus;
            dj["event_bonus_rate"] = deck.eventBonus.value_or(0);
            dj["support_deck_bonus_rate"] = deck.supportDeckBonus.value_or(0);
            dj["multi_live_score_up"] = deck.multiLiveScoreUp;

            if (deck.supportDeckCards.has_value()) {
                json support_cards_json = json::array();
                for (const auto& supportCard : deck.supportDeckCards.value()) {
                    json scj;
                    scj["card_id"] = supportCard.cardId;
                    scj["bonus"] = supportCard.supportDeckBonus.value_or(0);
                    support_cards_json.push_back(scj);
                }
                dj["support_deck_cards"] = support_cards_json;
            }

            json cards_json = json::array();
            for (const auto& card : deck.cards) {
                json cj;
                cj["card_id"] = card.cardId;
                cj["total_power"] = card.power.total;
                cj["base_power"] = card.power.base;
                cj["area_item_bonus_power"] = card.power.areaItemBonus;
                cj["character_bonus_power"] = card.power.characterBonus;
                cj["fixture_bonus_power"] = card.power.fixtureBonus;
                cj["gate_bonus_power"] = card.power.gateBonus;
                cj["event_bonus_rate"] = card.eventBonus.value_or(0);
                cj["master_rank"] = card.masterRank;
                cj["level"] = card.level;
                cj["skill_level"] = card.skillLevel;
                cj["skill_score_up"] = card.skill.scoreUp;
                cj["skill_life_recovery"] = card.skill.lifeRecovery;
                cj["episode1_read"] = card.episode1Read;
                cj["episode2_read"] = card.episode2Read;
                cj["after_training"] = card.afterTraining;
                cj["default_image"] = mappedEnumToString(EnumMap::defaultImage, card.defaultImage);
                cj["has_canvas_bonus"] = card.hasCanvasBonus;
                cards_json.push_back(cj);
            }
            dj["cards"] = cards_json;
            decks_json.push_back(dj);
        }

        json result_json;
        result_json["decks"] = decks_json;
        return result_json;
    }
};

// ---- C API implementation ----

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
        return static_cast<DeckRecommendHandle>(new SekaiDeckRecommendC());
    } catch (...) {
        return nullptr;
    }
}

void deck_recommend_destroy(DeckRecommendHandle handle) {
    delete static_cast<SekaiDeckRecommendC*>(handle);
}

const char* deck_recommend_update_masterdata(DeckRecommendHandle handle, const char* base_dir, const char* region) {
    try {
        static_cast<SekaiDeckRecommendC*>(handle)->update_masterdata(base_dir, region);
        return nullptr;
    } catch (const std::exception& e) {
        return alloc_error(e.what());
    }
}

const char* deck_recommend_update_masterdata_from_json(DeckRecommendHandle handle, const char* json_map, const char* region) {
    try {
        auto j = json::parse(json_map);
        std::map<std::string, std::string> data;
        for (auto& [key, val] : j.items()) {
            data[key] = val.is_string() ? val.get<std::string>() : val.dump();
        }
        static_cast<SekaiDeckRecommendC*>(handle)->update_masterdata_from_strings(data, region);
        return nullptr;
    } catch (const std::exception& e) {
        return alloc_error(e.what());
    }
}

const char* deck_recommend_update_musicmetas(DeckRecommendHandle handle, const char* file_path, const char* region) {
    try {
        static_cast<SekaiDeckRecommendC*>(handle)->update_musicmetas_file(file_path, region);
        return nullptr;
    } catch (const std::exception& e) {
        return alloc_error(e.what());
    }
}

const char* deck_recommend_update_musicmetas_from_string(DeckRecommendHandle handle, const char* json_str, const char* region) {
    try {
        static_cast<SekaiDeckRecommendC*>(handle)->update_musicmetas_string(json_str, region);
        return nullptr;
    } catch (const std::exception& e) {
        return alloc_error(e.what());
    }
}

const char* deck_recommend_cache_userdata(DeckRecommendHandle handle, const char* userdata_json, const char** hash_out) {
    try {
        auto userdata_hash = static_cast<SekaiDeckRecommendC*>(handle)->cache_userdata(userdata_json);
        if (hash_out) {
            *hash_out = alloc_cstr(userdata_hash);
        }
        return nullptr;
    } catch (const std::exception& e) {
        return alloc_error(e.what());
    }
}

const char* deck_recommend_recommend(DeckRecommendHandle handle, const char* options_json, const char** error_out) {
    try {
        auto opts = json::parse(options_json);
        auto result = static_cast<SekaiDeckRecommendC*>(handle)->recommend(opts);
        std::string s = result.dump();
        return alloc_cstr(s);
    } catch (const std::exception& e) {
        if (error_out) *error_out = alloc_error(e.what());
        return nullptr;
    }
}

const char* deck_recommend_calculate(DeckRecommendHandle handle, const char* options_json, const char** error_out) {
    try {
        auto opts = json::parse(options_json);
        auto result = static_cast<SekaiDeckRecommendC*>(handle)->calculate(opts);
        std::string s = result.dump();
        return alloc_cstr(s);
    } catch (const std::exception& e) {
        if (error_out) *error_out = alloc_error(e.what());
        return nullptr;
    }
}

void deck_recommend_free_string(const char* str) {
    free(const_cast<char*>(str));
}

} // extern "C"
