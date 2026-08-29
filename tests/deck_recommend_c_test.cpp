#define DECK_BRIDGE_UNIT_TEST
#include "../cpp_bridge/deck_recommend_c.cpp"

#include <cassert>
#include <cmath>
#include <cstdio>
#include <fstream>
#include <iostream>

namespace {

template <typename Fn>
void expect_error(Fn&& fn, std::string_view expected) {
    try {
        fn();
        assert(false && "expected an exception");
    } catch (const std::exception& error) {
        assert(std::string_view(error.what()).find(expected) != std::string_view::npos);
    }
}

template <typename Fn>
void expect_any_error(Fn&& fn) {
    try {
        fn();
        assert(false && "expected an exception");
    } catch (const std::exception&) {
    }
}

void expect_c_error(const char* error, std::string_view expected) {
    assert(error != nullptr);
    assert(std::string_view(error).find(expected) != std::string_view::npos);
    deck_recommend_free_string(error);
}

std::string minimal_userdata() {
    return R"({
        "userGamedata":{"userId":1},
        "userAreas":[],
        "userCards":[],
        "userChallengeLiveSoloDecks":[],
        "userCharacters":[],
        "userDecks":[],
        "userHonors":[],
        "userMysekaiCanvases":[],
        "userMysekaiFixtureGameCharacterPerformanceBonuses":[],
        "userMysekaiGates":[]
    })";
}

json_doc parse(std::string_view text) {
    return parse_json_bytes(text.data(), text.size(), "unit test JSON");
}

void test_json_helpers() {
    assert(hash_userdata_payload("") == "cbf29ce484222325");
    assert(ends_with_json_suffix("cards.json"));
    assert(ends_with_json_suffix("cards.JSON"));
    assert(!ends_with_json_suffix("json"));
    assert(normalize_masterdata_key("dir\\cards.JSON") == "cards");
    assert(normalize_masterdata_key("cards") == "cards");

    auto doc = parse(R"({"string":"value","array":[1],"number":1,"real":1.5,"bool":true,"null":null})");
    auto root = doc.root();
    assert(extract_json_string_or_dump(root["string"]) == "value");
    assert(extract_json_string_or_dump(root["array"]) == "[1]");
    assert(json_type_name(root) == "object");
    assert(json_type_name(root["array"]) == "array");
    assert(json_type_name(root["string"]) == "string");
    assert(json_type_name(root["number"]) == "integer");
    assert(json_type_name(root["real"]) == "other");
    assert(json_type_name(root["null"]) == "null");
    assert(json_opt<int>(root, "number") == 1);
    assert(!json_opt<int>(root, "missing").has_value());
    assert(context_value_or_json(root, "string", "forced", 6, "value") == "forced");
    assert(context_value_or_json(root, "string", nullptr, 0, "value") == "value");
    expect_error(
        [&] { context_value_or_json(root, "missing", nullptr, 0, "value"); },
        "value is required"
    );
    expect_error([&] { require_string_field(root, "missing"); }, "missing is required");
    expect_error([&] { require_int_field(root, "string"); }, "string is required");
    assert(has_int_field(root, "number"));
    assert(!has_int_field(root, "null"));

    expect_error(
        [] { parse_json_bytes(nullptr, 0, "empty"); },
        "empty JSON input"
    );
}

DeckDetail sample_deck_detail() {
    DeckDetail detail{};
    detail.power = {100, 20, 30, 40, 50, 60, 300};
    detail.eventBonus = 25.5;
    detail.supportDeckBonus = 12.5;
    detail.multiLiveScoreUp = 130.5;

    DeckCardDetail card{};
    card.cardId = 10;
    card.level = 60;
    card.skillLevel = 4;
    card.masterRank = 5;
    card.power = {100, 20, 30, 40, 50, 240};
    card.eventBonus = 25.5;
    card.skill.scoreUp = 100.5;
    card.skill.lifeRecovery = 500;
    card.episode1Read = true;
    card.episode2Read = true;
    card.afterTraining = true;
    card.defaultImage = mapEnum(EnumMap::defaultImage, "special_training");
    card.hasCanvasBonus = true;
    detail.cards.push_back(card);

    CardDetail support{};
    support.cardId = 20;
    support.supportDeckBonus = 12.5;
    support.skillLevel = 3;
    support.masterRank = 2;
    support.level = 50;
    support.afterTraining = false;
    support.defaultImage = mapEnum(EnumMap::defaultImage, "original");
    detail.supportDeckCards = std::vector<CardDetail>{support};
    return detail;
}

void test_result_serializers() {
    auto detail = sample_deck_detail();

    MutableJsonDoc detail_doc;
    auto detail_json = dump_mutable_json(deck_detail_to_json(detail_doc.get(), detail));
    assert(detail_json.find("\"eventBonus\":25.5") != std::string::npos);
    assert(detail_json.find("\"cardId\":10") != std::string::npos);

    LiveDetail live{123456, 120.5, 1000, 900, detail};
    MutableJsonDoc live_doc;
    auto live_json = dump_mutable_json(live_detail_to_json(live_doc.get(), live));
    assert(live_json.find("\"score\":123456") != std::string::npos);
    assert(live_json.find("\"deck\"") != std::string::npos);

    RecommendDeck recommendation{};
    static_cast<DeckDetail&>(recommendation) = detail;
    recommendation.score = 500000;
    recommendation.liveScore = 400000;
    recommendation.mysekaiEventPoint = 300000;
    recommendation.multiLiveScoreUp = 130.5;

    MutableJsonDoc recommendation_doc;
    auto recommendation_json = dump_mutable_json(
        recommend_deck_to_json(recommendation_doc.get(), recommendation)
    );
    assert(recommendation_json.find("\"support_deck_cards\"") != std::string::npos);
    assert(recommendation_json.find("\"cards\"") != std::string::npos);

    auto result = SekaiDeckRecommendC::serialize_recommendation_result(
        {recommendation},
        1.25
    );
    assert(result.find("\"decks\"") != std::string::npos);
    assert(result.find("\"cost_ms\":1.25") != std::string::npos);
}

void test_live_skill_parser() {
    auto empty = parse(R"({})");
    assert(!parse_live_skills(empty.root()).has_value());

    auto valid = parse(R"({"skills":[{"seq":1,"cardId":10},{"card_id":20}]})");
    auto skills = parse_live_skills(valid.root());
    assert(skills.has_value());
    assert(skills->size() == 2);
    assert(skills->at(0).seq == 1);
    assert(skills->at(1).cardId == 20);

    auto not_array = parse(R"({"skills":{}})");
    expect_error([&] { parse_live_skills(not_array.root()); }, "must be an array");
    auto bad_entry = parse(R"({"skills":[1]})");
    expect_error([&] { parse_live_skills(bad_entry.root()); }, "must be objects");
    auto missing_card = parse(R"({"skills":[{}]})");
    expect_error([&] { parse_live_skills(missing_card.root()); }, "require cardId");
}

void test_configuration_helpers() {
    auto masterdata = std::make_shared<MasterData>();
    Card card{};
    card.id = 10;
    masterdata->cards.push_back(card);
    Event event{};
    event.id = 20;
    masterdata->events.push_back(event);

    auto musicmetas = std::make_shared<MusicMetas>();
    MusicMeta meta{};
    meta.music_id = 1;
    meta.difficulty = mapEnum(EnumMap::musicDifficulty, "master");
    musicmetas->metas.push_back(meta);
    DataProvider provider{
        Region::JP,
        masterdata,
        std::make_shared<UserData>(),
        musicmetas,
    };

    auto options_doc = parse(R"({
        "target":"power",
        "algorithm":"dfs-ga",
        "music_id":1,
        "music_diff":"master",
        "limit":2,
        "member":5,
        "fixed_cards":[10],
        "fixed_characters":[1,2],
        "forcedLeaderCharacterId":3,
        "skill_reference_choose_strategy":"min",
        "skill_order_choose_strategy":"min",
        "specific_skill_order":[4,3,2,1,0],
        "multi_live_teammate_score_up":100,
        "multi_live_teammate_power":200000,
        "multi_live_score_up_lower_bound":10.5,
        "rarity_1_config":{"level":20},
        "single_card_configs":[{"card_id":10,"skill_level":4}],
        "support_master_max":true,
        "support_skill_max":true,
        "sa_options":{
            "run_num":2,
            "seed":1,
            "max_iter":2,
            "max_no_improve_iter":2,
            "time_limit_ms":1,
            "start_temprature":2.0,
            "cooling_rate":0.5,
            "debug":true
        },
        "ga_options":{
            "seed":1,
            "debug":true,
            "max_iter":2,
            "max_no_improve_iter":2,
            "pop_size":4,
            "parent_size":2,
            "elite_size":1,
            "crossover_rate":0.5,
            "base_mutation_rate":0.2,
            "no_improve_iter_to_mutation_rate":0.3
        }
    })");
    auto options = options_doc.root();

    DeckRecommendConfig config{};
    SekaiDeckRecommendC::apply_target_options(config, options, false);
    assert(config.target == RecommendTarget::Power);
    SekaiDeckRecommendC::apply_target_options(config, options, true);
    assert(config.target == RecommendTarget::Mysekai);
    SekaiDeckRecommendC::apply_algorithm_options(config, options, false);
    assert(config.algorithm == RecommendAlgorithm::DFS_GA);
    SekaiDeckRecommendC::apply_music_and_size_options(config, options, provider);
    SekaiDeckRecommendC::apply_fixed_card_options(config, options, provider);
    SekaiDeckRecommendC::apply_fixed_character_options(config, options, false);
    SekaiDeckRecommendC::apply_forced_leader_option(config, options);
    SekaiDeckRecommendC::apply_skill_strategy_options(config, options);
    SekaiDeckRecommendC::apply_multi_live_options(
        config,
        options,
        mapEnum(EnumMap::liveType, "multi")
    );
    SekaiDeckRecommendC::apply_card_options(config, options);
    SekaiDeckRecommendC::apply_sa_options(config, options);
    SekaiDeckRecommendC::apply_ga_options(config, options);
    SekaiDeckRecommendC::apply_timeout_option(config, options, 6000);
    assert(config.timeout_ms == kMaxRecommendTimeoutMs);
    assert(config.fixedCards == std::vector<int>{10});
    assert(config.forcedLeaderCharacterId == 3);

    auto score = parse(R"({"target":"score"})");
    SekaiDeckRecommendC::apply_target_options(config, score.root(), false);
    assert(config.target == RecommendTarget::Score);
    auto skill = parse(R"({"target":"skill"})");
    SekaiDeckRecommendC::apply_target_options(config, skill.root(), false);
    assert(config.target == RecommendTarget::Skill);
    auto bonus = parse(R"({"target":"bonus","target_bonus_list":[10]})");
    SekaiDeckRecommendC::apply_target_options(config, bonus.root(), false);
    assert(config.target == RecommendTarget::Bonus);

    for (const auto& name : {"sa", "dfs", "ga", "dfs_ga", "rl"}) {
        auto algorithm = parse(std::string("{\"algorithm\":\"") + name + "\"}");
        SekaiDeckRecommendC::apply_algorithm_options(config, algorithm.root(), false);
    }

    for (const auto& name : {"average", "max", "min"}) {
        auto strategy = parse(
            std::string("{\"skill_reference_choose_strategy\":\"") + name
            + "\",\"skill_order_choose_strategy\":\"" + name + "\"}"
        );
        SekaiDeckRecommendC::apply_skill_strategy_options(config, strategy.root());
    }

    assert(SekaiDeckRecommendC::resolve_region("jp") == Region::JP);
    expect_error(
        [] { SekaiDeckRecommendC::resolve_region("invalid"); },
        "Invalid region"
    );
    auto live = parse(R"({"live_type":"mysekai"})");
    auto context = SekaiDeckRecommendC::resolve_live_context(live.root());
    assert(context.is_mysekai);
    assert(!context.is_challenge);
    assert(SekaiDeckRecommendC::resolve_event_id(options, provider, true) == 0);
    auto event_options = parse(R"({"event_id":20})");
    assert(SekaiDeckRecommendC::resolve_event_id(event_options.root(), provider, false) == 20);
    assert(SekaiDeckRecommendC::resolve_challenge_character_id(options, false) == 0);
    assert(SekaiDeckRecommendC::resolve_world_bloom_character_id(options, provider, 0) == 0);
    assert(SekaiDeckRecommendC::resolve_support_event_id(event_options.root(), *masterdata) == 20);

    auto support_character = parse(R"({"forced_leader_character_id":21})");
    assert(SekaiDeckRecommendC::resolve_support_character_id(support_character.root()) == 21);
    auto support_character_camel = parse(R"({"forcedLeaderCharacterId":22})");
    assert(SekaiDeckRecommendC::resolve_support_character_id(support_character_camel.root()) == 22);
}

void test_validation_paths() {
    auto masterdata = std::make_shared<MasterData>();
    WorldBloom world_bloom{};
    world_bloom.eventId = 20;
    world_bloom.gameCharacterId = 21;
    masterdata->worldBlooms.push_back(world_bloom);

    DataProvider provider{
        Region::JP,
        masterdata,
        std::make_shared<UserData>(),
        std::make_shared<MusicMetas>(),
    };
    DeckRecommendConfig config{};

    auto empty = parse(R"({})");
    SekaiDeckRecommendC::apply_custom_bonus_support_units(config, empty.root());
    SekaiDeckRecommendC::apply_fixed_card_options(config, empty.root(), provider);
    SekaiDeckRecommendC::apply_fixed_character_options(config, empty.root(), false);
    SekaiDeckRecommendC::apply_forced_leader_option(config, empty.root());
    SekaiDeckRecommendC::apply_sa_options(config, empty.root());
    SekaiDeckRecommendC::apply_ga_options(config, empty.root());
    expect_error(
        [&] { SekaiDeckRecommendC::resolve_live_context(empty.root()); },
        "live_type is required"
    );

    auto bad_support_key = parse(
        R"({"custom_bonus_character_support_units":{"21x":"leo_need"}})"
    );
    expect_error(
        [&] {
            SekaiDeckRecommendC::apply_custom_bonus_support_units(
                config,
                bad_support_key.root()
            );
        },
        "Invalid custom bonus support unit character ID key"
    );

    auto challenge_character = parse(R"({"challenge_live_character_id":21})");
    assert(
        SekaiDeckRecommendC::resolve_challenge_character_id(
            challenge_character.root(),
            true
        ) == 21
    );

    auto invalid_world_bloom_character = parse(R"({"world_bloom_character_id":0})");
    expect_error(
        [&] {
            SekaiDeckRecommendC::resolve_world_bloom_character_id(
                invalid_world_bloom_character.root(),
                provider,
                20
            );
        },
        "Invalid world bloom character ID"
    );
    auto world_bloom_character = parse(R"({"world_bloom_character_id":21})");
    assert(
        SekaiDeckRecommendC::resolve_world_bloom_character_id(
            world_bloom_character.root(),
            provider,
            20
        ) == 21
    );

    auto third_turn = parse(
        R"({"world_bloom_event_turn":3,"world_bloom_character_id":21})"
    );
    assert(
        SekaiDeckRecommendC::resolve_world_bloom_fake_event_id(
            third_turn.root(),
            provider
        ) > 0
    );
    auto first_turn = parse(
        R"({"world_bloom_event_turn":1,"event_unit":"idol"})"
    );
    assert(
        SekaiDeckRecommendC::resolve_world_bloom_fake_event_id(
            first_turn.root(),
            provider
        ) > 0
    );
    auto unit_attr_event = parse(
        R"({"event_type":"marathon","event_attr":"cool","event_unit":"idol"})"
    );
    assert(
        SekaiDeckRecommendC::resolve_fake_event_id(
            unit_attr_event.root(),
            provider
        ) > 0
    );

    expect_error(
        [&] { SekaiDeckRecommendC::resolve_support_event_id(empty.root(), *masterdata); },
        "event_id or world_bloom_event_turn is required"
    );
    auto invalid_turn = parse(R"({"world_bloom_event_turn":0})");
    expect_error(
        [&] {
            SekaiDeckRecommendC::resolve_support_event_id(
                invalid_turn.root(),
                *masterdata
            );
        },
        "Invalid world bloom event turn"
    );
    auto third_turn_without_character = parse(R"({"world_bloom_event_turn":3})");
    expect_error(
        [&] {
            SekaiDeckRecommendC::resolve_support_event_id(
                third_turn_without_character.root(),
                *masterdata
            );
        },
        "world_bloom_character_id is required"
    );
    auto first_turn_without_unit = parse(R"({"world_bloom_event_turn":1})");
    expect_error(
        [&] {
            SekaiDeckRecommendC::resolve_support_event_id(
                first_turn_without_unit.root(),
                *masterdata
            );
        },
        "event_unit is required"
    );
    auto invalid_unit = parse(
        R"({"world_bloom_event_turn":1,"event_unit":"invalid"})"
    );
    expect_error(
        [&] {
            SekaiDeckRecommendC::resolve_support_event_id(
                invalid_unit.root(),
                *masterdata
            );
        },
        "Invalid event unit"
    );
    assert(
        SekaiDeckRecommendC::resolve_support_event_id(
            third_turn.root(),
            *masterdata
        ) > 0
    );
    assert(
        SekaiDeckRecommendC::resolve_support_event_id(
            first_turn.root(),
            *masterdata
        ) > 0
    );

    auto challenge_fixed_character = parse(R"({"fixed_characters":[1]})");
    expect_error(
        [&] {
            SekaiDeckRecommendC::apply_fixed_character_options(
                config,
                challenge_fixed_character.root(),
                true
            );
        },
        "fixed_characters is not valid for challenge live"
    );

    expect_error(
        [&] { SekaiDeckRecommendC::resolve_support_character_id(empty.root()); },
        "world_bloom_character_id or forcedLeaderCharacterId is required"
    );
    expect_error(
        [&] {
            SekaiDeckRecommendC::resolve_support_character_id(
                invalid_world_bloom_character.root()
            );
        },
        "Invalid world_bloom_character_id or forcedLeaderCharacterId"
    );

    SekaiDeckRecommendC bridge;
    expect_error(
        [&] { bridge.update_masterdata("", "invalid"); },
        "Invalid region"
    );
    expect_error(
        [&] { bridge.update_musicmetas_file("", "invalid"); },
        "Invalid region"
    );
    expect_error(
        [&] { bridge.attach_cached_userdata(""); },
        "userdata_hash is required"
    );
}

void test_userdata_and_calculation_paths() {
    SekaiDeckRecommendC bridge;
    auto empty = parse(R"({})");
    expect_error(
        [&] { bridge.resolve_userdata(empty.root()); },
        "Either userdata_hash"
    );
    expect_error(
        [&] { bridge.resolve_userdata(empty.root(), "missing", 7); },
        "User data not found"
    );

    auto unknown_hash = parse(R"({"userdata_hash":"missing"})");
    expect_error(
        [&] { bridge.resolve_userdata(unknown_hash.root()); },
        "User data not found"
    );

    auto direct_userdata = parse(
        std::string(R"({"user_data":)") + minimal_userdata() + "}"
    );
    auto userdata = bridge.resolve_userdata(direct_userdata.root());
    assert(userdata != nullptr);

    auto userdata_string = parse(R"({"user_data_str":"not-json"})");
    expect_error(
        [&] { bridge.resolve_userdata(userdata_string.root()); },
        "Failed to load user data from bytes"
    );

    const char* userdata_path = "/tmp/deck-service-userdata-unit.json";
    {
        std::ofstream userdata_file(userdata_path);
        userdata_file << minimal_userdata();
    }
    auto userdata_file = parse(
        std::string(R"({"user_data_file_path":")") + userdata_path + R"("})"
    );
    auto loaded_userdata = bridge.resolve_userdata(userdata_file.root());
    assert(loaded_userdata->path == userdata_path);
    assert(std::remove(userdata_path) == 0);

    auto invalid_region = parse(R"({"region":"invalid"})");
    expect_error(
        [&] { bridge.build_data_provider(invalid_region.root(), false); },
        "Invalid region"
    );

    auto missing_masterdata = parse(
        std::string(R"({"region":"cn","user_data":)") + minimal_userdata() + "}"
    );
    expect_error(
        [&] { bridge.build_data_provider(missing_masterdata.root(), false); },
        "Master data not found"
    );

    shared_region_data_store().set_masterdata(
        Region::KR,
        std::make_shared<MasterData>()
    );
    auto provider_options = parse(
        std::string(R"({"region":"kr","user_data":)") + minimal_userdata() + "}"
    );
    auto provider = bridge.build_data_provider(provider_options.root(), false);
    assert(provider.region == Region::KR);

    shared_region_data_store().set_masterdata(
        Region::EN,
        std::make_shared<MasterData>()
    );
    auto missing_musicmetas = parse(
        std::string(R"({"region":"en","user_data":)") + minimal_userdata() + "}"
    );
    expect_error(
        [&] { bridge.build_data_provider(missing_musicmetas.root(), true); },
        "Music metas not found"
    );

    auto challenge = parse(R"({})");
    expect_error(
        [&] { bridge.resolve_fixed_deck_cards(provider, challenge.root(), "challenge"); },
        "character_id is required"
    );
    expect_error(
        [&] { bridge.resolve_fixed_deck_cards(provider, challenge.root(), "deck"); },
        "deck_id is required"
    );
    expect_error(
        [&] { bridge.resolve_fixed_deck_cards(provider, challenge.root(), "live_full"); },
        "Either deck_id or character_id is required"
    );

    UserCard card{};
    card.userId = 1;
    card.cardId = 1;
    provider.userData->userCards.push_back(card);
    UserChallengeLiveSoloDeck challenge_deck{};
    challenge_deck.characterId = 21;
    challenge_deck.leader = 1;
    provider.userData->userChallengeLiveSoloDecks.push_back(challenge_deck);
    UserDeck deck{};
    deck.deckId = 1;
    deck.member1 = 1;
    deck.member2 = 1;
    deck.member3 = 1;
    deck.member4 = 1;
    deck.member5 = 1;
    provider.userData->userDecks.push_back(deck);

    auto challenge_options = parse(R"({"character_id":21})");
    assert(
        bridge.resolve_fixed_deck_cards(
            provider,
            challenge_options.root(),
            "challenge"
        ).size() == 1
    );
    assert(
        bridge.resolve_fixed_deck_cards(
            provider,
            challenge_options.root(),
            "live_full"
        ).size() == 1
    );
    auto deck_options = parse(R"({"deck_id":1})");
    assert(
        bridge.resolve_fixed_deck_cards(
            provider,
            deck_options.root(),
            "deck"
        ).size() == 5
    );
    assert(
        bridge.resolve_fixed_deck_cards(
            provider,
            deck_options.root(),
            "live_full"
        ).size() == 5
    );
    expect_error(
        [&] { bridge.calculate_fixed_deck_detail(provider, {}); },
        "fixed deck contains no cards"
    );

    expect_any_error(
        [&] { bridge.update_masterdata("/path/that/does/not/exist", "cn"); }
    );
    expect_any_error(
        [&] { bridge.update_musicmetas_file("/path/that/does/not/exist", "cn"); }
    );

    SekaiDeckRecommendC producer;
    auto userdata_hash = producer.cache_userdata(minimal_userdata());
    SekaiDeckRecommendC consumer;
    auto invalid_batch_entry = parse(R"([1])");
    expect_error(
        [&] {
            consumer.recommend_batch(
                invalid_batch_entry.root(),
                0,
                "jp",
                userdata_hash.c_str(),
                2,
                userdata_hash.size()
            );
        },
        "entries must be objects"
    );
}

void test_legacy_update_entry_points() {
    expect_c_error(
        deck_recommend_update_masterdata(nullptr, "", "jp"),
        "handle is required"
    );
    expect_c_error(
        deck_recommend_update_masterdata_from_json(nullptr, "[]", "jp"),
        "must be an object"
    );
    expect_c_error(
        deck_recommend_update_musicmetas(nullptr, "", "jp"),
        "handle is required"
    );
    expect_c_error(
        deck_recommend_update_musicmetas_from_string(nullptr, "", "jp"),
        "Failed to load music metas"
    );

    const char* hash = nullptr;
    expect_c_error(
        deck_recommend_cache_userdata(nullptr, "", &hash),
        "userdata_json is required"
    );
    expect_c_error(
        deck_recommend_attach_cached_userdata(nullptr, ""),
        "userdata_hash is required"
    );
}

} // namespace

int main() {
    test_json_helpers();
    test_result_serializers();
    test_live_skill_parser();
    test_configuration_helpers();
    test_validation_paths();
    test_userdata_and_calculation_paths();
    test_legacy_update_entry_points();
    std::cout << "C++ bridge unit tests passed\n";
    return 0;
}
