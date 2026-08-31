#![recursion_limit = "256"]

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;

use deck_service::bridge::DeckRecommend;
use deck_service::ffi;
use sonic_rs::{Value, json};

const MASTERDATA_KEYS: &[&str] = &[
    "areaItemLevels",
    "areaItems",
    "areas",
    "cardEpisodes",
    "cards",
    "cardRarities",
    "characterRanks",
    "eventCards",
    "eventDeckBonuses",
    "eventExchangeSummaries",
    "events",
    "eventItems",
    "eventRarityBonusRates",
    "gameCharacters",
    "gameCharacterUnits",
    "honors",
    "masterLessons",
    "musicDifficulties",
    "musics",
    "musicVocals",
    "shopItems",
    "skills",
    "worldBloomDifferentAttributeBonuses",
    "worldBlooms",
    "worldBloomSupportDeckBonuses",
    "worldBloomSupportDeckUnitEventLimitedBonuses",
    "cardMysekaiCanvasBonuses",
    "eventCardBonusLimits",
    "eventHonorBonuses",
    "eventMysekaiFixtureGameCharacterPerformanceBonusLimits",
    "eventSkillScoreUpLimits",
    "ingameCombos",
    "ingameNotes",
    "mysekaiFixtureGameCharacterGroups",
    "mysekaiFixtureGameCharacterGroupPerformanceBonuses",
    "mysekaiGates",
    "mysekaiGateLevels",
];

fn minimal_masterdata() -> HashMap<String, String> {
    let mut data = MASTERDATA_KEYS
        .iter()
        .map(|key| ((*key).to_owned(), "[]".to_owned()))
        .collect::<HashMap<_, _>>();

    // Exercise path and case normalization while retaining the canonical key,
    // which must take precedence when both forms are supplied.
    data.insert("nested/cards.JSON".to_owned(), "[{}]".to_owned());
    data.insert("x".to_owned(), "[]".to_owned());
    data
}

fn userdata(user_id: usize) -> String {
    sonic_rs::to_string(&json!({
        "userGamedata": {"userId": user_id},
        "userAreas": [],
        "userCards": [],
        "userChallengeLiveSoloDecks": [],
        "userCharacters": [],
        "userDecks": [],
        "userHonors": [],
        "userMysekaiCanvases": [],
        "userMysekaiFixtureGameCharacterPerformanceBonuses": [],
        "userMysekaiGates": []
    }))
    .unwrap()
}

fn base_options(userdata_hash: &str) -> Value {
    json!({
        "region": "jp",
        "userdata_hash": userdata_hash,
        "live_type": "multi",
        "target": "bonus",
        "target_bonus_list": [100, 200],
        "custom_bonus_attr": "cool",
        "custom_bonus_character_ids": [21],
        "custom_bonus_character_support_units": {"21": "light_sound"},
        "algorithm": "ga",
        "filter_other_unit": true,
        "music_id": 1,
        "music_diff": "master",
        "limit": 1,
        "member": 5,
        "fixed_cards": [],
        "fixed_characters": [],
        "forced_leader_character_id": 1,
        "skill_reference_choose_strategy": "max",
        "skill_order_choose_strategy": "specific",
        "specific_skill_order": [1, 2, 3, 4, 5],
        "keep_after_training_state": true,
        "multi_live_teammate_score_up": 100,
        "multi_live_teammate_power": 200000,
        "multi_live_score_up_lower_bound": 10.5,
        "best_skill_as_leader": false,
        "timeout_ms": 0,
        "rarity_1_config": {
            "disable": false,
            "level_max": true,
            "episode_read": true,
            "master_max": true,
            "skill_max": true,
            "canvas": false,
            "level": 20,
            "skill_level": 4,
            "master_rank": 5,
            "episode_read_count": 2
        },
        "single_card_configs": [{
            "card_id": 1,
            "disable": true,
            "level": 1
        }],
        "support_master_max": true,
        "support_skill_max": true,
        "sa_options": {
            "run_num": 1,
            "seed": 7,
            "max_iter": 1,
            "max_no_improve_iter": 1,
            "time_limit_ms": 0,
            "start_temperature": 1.0,
            "cooling_rate": 0.5,
            "debug": false
        },
        "ga_options": {
            "seed": 7,
            "debug": false,
            "max_iter": 1,
            "max_no_improve_iter": 1,
            "pop_size": 2,
            "parent_size": 1,
            "elite_size": 1,
            "crossover_rate": 0.5,
            "base_mutation_rate": 0.1,
            "no_improve_iter_to_mutation_rate": 0.2
        }
    })
}

fn call_recommend(engine: &DeckRecommend, options: &Value) -> Result<String, String> {
    engine.recommend_raw(&sonic_rs::to_string(options).unwrap())
}

fn expect_recommend_error(engine: &DeckRecommend, options: Value, expected: &str) {
    let error = call_recommend(engine, &options).unwrap_err();
    assert!(
        error.contains(expected),
        "expected error containing {expected:?}, got {error:?}"
    );
}

fn changed(mut options: Value, key: &str, value: Value) -> Value {
    options.insert(key, value);
    options
}

#[test]
fn bridge_validates_recommendation_options_and_shared_caches() {
    DeckRecommend::init_data_path(concat!(env!("CARGO_MANIFEST_DIR"), "/_cpp_src/data")).unwrap();

    let masterdata = minimal_masterdata();
    let engine = DeckRecommend::new().unwrap();
    assert!(
        engine
            .update_masterdata_from_json(&masterdata, "invalid")
            .unwrap_err()
            .contains("Invalid region")
    );
    engine
        .update_masterdata_from_json(&masterdata, "jp")
        .unwrap();

    assert!(
        engine
            .update_musicmetas_from_string("[]", "invalid")
            .unwrap_err()
            .contains("Invalid region")
    );
    assert!(
        engine
            .update_musicmetas_from_string("not-json", "jp")
            .unwrap_err()
            .contains("Failed to load music metas")
    );
    engine
        .update_musicmetas_from_string(
            r#"[{"music_id":1,"difficulty":"master","music_time":120.0}]"#,
            "jp",
        )
        .unwrap();

    assert!(engine.cache_userdata("").unwrap_err().contains("required"));
    assert!(
        engine
            .cache_userdata("not-json")
            .unwrap_err()
            .contains("Failed to load user data")
    );

    let first_hash = engine.cache_userdata(&userdata(0)).unwrap();
    let mut active_hash = first_hash.clone();
    for user_id in 1..=65 {
        active_hash = engine.cache_userdata(&userdata(user_id)).unwrap();
    }

    let second_engine = DeckRecommend::new().unwrap();
    second_engine.attach_cached_userdata(&active_hash).unwrap();
    assert!(
        second_engine
            .attach_cached_userdata("")
            .unwrap_err()
            .contains("required")
    );
    assert!(
        second_engine
            .attach_cached_userdata("missing")
            .unwrap_err()
            .contains("not found")
    );

    // The complete option set reaches the native search after exercising every
    // successful configuration parser. An empty synthetic card pool is allowed
    // to return either an empty result or a domain error from the search engine.
    let base = base_options(&active_hash);
    let _ = call_recommend(&second_engine, &base);
    let _ = second_engine
        .recommend_raw_with_default_timeout(&sonic_rs::to_string(&base).unwrap(), Some(6000));
    let _ = second_engine.recommend_raw_with_context(
        &sonic_rs::to_string(&base).unwrap(),
        Some("jp"),
        Some(&active_hash),
        Some(10),
    );

    expect_recommend_error(
        &second_engine,
        changed(base.clone(), "region", json!("bad")),
        "Invalid region",
    );
    expect_recommend_error(
        &second_engine,
        changed(base.clone(), "userdata_hash", json!(7)),
        "userdata_hash must be a string",
    );
    expect_recommend_error(
        &second_engine,
        changed(base.clone(), "live_type", json!("bad")),
        "Invalid live type",
    );
    expect_recommend_error(
        &second_engine,
        changed(base.clone(), "event_type", json!("bad")),
        "Invalid event type",
    );
    expect_recommend_error(
        &second_engine,
        changed(base.clone(), "event_attr", json!("cool")),
        "must be specified together",
    );
    expect_recommend_error(
        &second_engine,
        changed(
            changed(base.clone(), "event_attr", json!("bad")),
            "event_unit",
            json!("idol"),
        ),
        "Invalid event attr",
    );
    expect_recommend_error(
        &second_engine,
        changed(
            changed(base.clone(), "event_attr", json!("cool")),
            "event_unit",
            json!("bad"),
        ),
        "Invalid event unit",
    );
    expect_recommend_error(
        &second_engine,
        changed(base.clone(), "world_bloom_event_turn", json!(0)),
        "Invalid world bloom event turn",
    );
    expect_recommend_error(
        &second_engine,
        changed(base.clone(), "world_bloom_event_turn", json!(1)),
        "event_unit is required",
    );
    expect_recommend_error(
        &second_engine,
        changed(base.clone(), "world_bloom_event_turn", json!(3)),
        "world_bloom_character_id is required",
    );
    expect_recommend_error(
        &second_engine,
        changed(base.clone(), "world_bloom_finale_turn", json!(1)),
        "Invalid world bloom finale turn",
    );
    let finale = changed(
        changed(base.clone(), "world_bloom_finale_turn", json!(3)),
        "world_bloom_character_id",
        json!(21),
    );
    if let Err(error) = call_recommend(&second_engine, &finale) {
        assert!(!error.contains("World bloom chapter not found"));
    }

    let challenge = changed(base.clone(), "live_type", json!("challenge"));
    expect_recommend_error(
        &second_engine,
        challenge.clone(),
        "challenge_live_character_id is required",
    );
    expect_recommend_error(
        &second_engine,
        changed(challenge.clone(), "challenge_live_character_id", json!(27)),
        "Invalid challenge character ID",
    );
    expect_recommend_error(
        &second_engine,
        changed(
            changed(challenge, "challenge_live_character_id", json!(1)),
            "event_id",
            json!(1),
        ),
        "event_id is not valid",
    );

    for (key, value, expected) in [
        ("target", json!("bad"), "Invalid target"),
        (
            "custom_bonus_attr",
            json!("bad"),
            "Invalid custom bonus attr",
        ),
        (
            "custom_bonus_character_ids",
            json!([0]),
            "Invalid custom bonus character ID",
        ),
        (
            "custom_bonus_character_ids",
            json!([21, 21]),
            "Duplicate custom bonus character ID",
        ),
        (
            "custom_bonus_character_support_units",
            json!([]),
            "must be an object",
        ),
        ("algorithm", json!("bad"), "Invalid algorithm"),
        ("music_diff", json!("bad"), "Invalid music difficulty"),
        ("music_id", json!(2), "Music meta not found"),
        ("limit", json!(0), "Invalid limit"),
        ("member", json!(1), "Invalid member count"),
        (
            "fixed_cards",
            json!([1, 2, 3, 4, 5, 6]),
            "Fixed cards size exceeds",
        ),
        ("fixed_cards", json!([999]), "Invalid fixed card ID"),
        (
            "fixed_characters",
            json!([1, 2, 3, 4, 5, 6]),
            "Fixed characters size exceeds",
        ),
        (
            "fixed_characters",
            json!([27]),
            "Invalid fixed character ID",
        ),
        (
            "forced_leader_character_id",
            json!(27),
            "Invalid forced leader character ID",
        ),
        (
            "skill_reference_choose_strategy",
            json!("bad"),
            "Invalid skill ref strategy",
        ),
        (
            "skill_order_choose_strategy",
            json!("bad"),
            "Invalid skill order strategy",
        ),
    ] {
        expect_recommend_error(&second_engine, changed(base.clone(), key, value), expected);
    }

    let score_target = changed(base.clone(), "target", json!("score"));
    expect_recommend_error(
        &second_engine,
        score_target,
        "target_bonus_list is only valid",
    );

    for (support_units, expected) in [
        (
            json!({"bad": "idol"}),
            "Invalid custom bonus support unit character ID key",
        ),
        (json!({"1": "idol"}), "only valid for virtual singer"),
        (json!({"21": "piapro"}), "Invalid custom bonus support unit"),
        (
            json!({"22": "idol"}),
            "must be included in custom_bonus_character_ids",
        ),
    ] {
        expect_recommend_error(
            &second_engine,
            changed(
                base.clone(),
                "custom_bonus_character_support_units",
                support_units,
            ),
            expected,
        );
    }

    let solo = changed(base.clone(), "live_type", json!("solo"));
    for (key, expected) in [
        (
            "multi_live_teammate_score_up",
            "multi_live_teammate_score_up is only valid",
        ),
        (
            "multi_live_teammate_power",
            "multi_live_teammate_power is only valid",
        ),
        (
            "multi_live_score_up_lower_bound",
            "multi_live_score_up_lower_bound is only valid",
        ),
    ] {
        let mut options = solo.clone();
        options.insert("multi_live_teammate_score_up", json!(null));
        options.insert("multi_live_teammate_power", json!(null));
        options.insert("multi_live_score_up_lower_bound", json!(null));
        options.insert(key, json!(1));
        expect_recommend_error(&second_engine, options, expected);
    }

    for (sa_options, expected) in [
        (json!({"run_num": 0}), "Invalid sa run count"),
        (json!({"max_iter": 0}), "Invalid sa max iter"),
        (
            json!({"max_no_improve_iter": 0}),
            "Invalid sa max no improve iter",
        ),
        (json!({"time_limit_ms": -1}), "Invalid sa max time ms"),
        (
            json!({"start_temprature": -1}),
            "Invalid sa start temperature",
        ),
        (json!({"cooling_rate": 2}), "Invalid sa cooling rate"),
    ] {
        expect_recommend_error(
            &second_engine,
            changed(base.clone(), "sa_options", sa_options),
            expected,
        );
    }

    let batch_json = sonic_rs::to_string(&json!([base.clone(), {"live_type": "bad"}])).unwrap();
    let batch = second_engine
        .recommend_batch_raw_with_context(&batch_json, "jp", &active_hash, Some(10))
        .unwrap();
    assert!(batch.contains("cost_time"));
    assert!(batch.contains("error"));
    assert!(
        second_engine
            .recommend_batch_raw_with_context("{}", "jp", &active_hash, None)
            .unwrap_err()
            .contains("must be an array")
    );
    assert!(
        second_engine
            .recommend_batch_raw_with_context("[]", "", &active_hash, None)
            .unwrap_err()
            .contains("region is required")
    );
    assert!(
        second_engine
            .recommend_batch_raw_with_context("[]", "jp", "", None)
            .unwrap_err()
            .contains("userdata_hash is required")
    );

    assert!(
        second_engine
            .calculate_raw(
                &sonic_rs::to_string(&json!({
                    "region": "jp",
                    "userdata_hash": active_hash,
                    "mode": "bad"
                }))
                .unwrap()
            )
            .unwrap_err()
            .contains("Invalid calculate mode")
    );
    let support = second_engine
        .get_world_bloom_support_cards_raw(
            &sonic_rs::to_string(&json!({
                "region": "jp",
                "userdata_hash": active_hash,
                "event_id": 0,
                "world_bloom_character_id": 21,
                "support_master_max": true,
                "support_skill_max": true,
                "filter_other_unit": true
            }))
            .unwrap(),
        )
        .unwrap();
    assert_eq!(support, "[]");

    let finale_support = second_engine
        .get_world_bloom_support_cards_raw(
            &sonic_rs::to_string(&json!({
                "region": "jp",
                "userdata_hash": active_hash,
                "world_bloom_finale_turn": 3,
                "forced_leader_character_id": 21
            }))
            .unwrap(),
        )
        .unwrap();
    assert_eq!(finale_support, "[]");
}

unsafe fn take_c_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let value = unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    unsafe { ffi::deck_recommend_free_string(ptr) };
    Some(value)
}

#[test]
fn legacy_c_entry_points_return_owned_errors() {
    let invalid_json = CString::new("not-json").unwrap();
    let region = CString::new("jp").unwrap();
    let mut error: *const c_char = std::ptr::null();

    unsafe {
        let handle = ffi::deck_recommend_create();
        assert!(!handle.is_null());

        let result = ffi::deck_recommend_recommend(handle, invalid_json.as_ptr(), &mut error);
        assert!(result.is_null());
        assert!(take_c_string(error).unwrap().contains("Failed to parse"));

        error = std::ptr::null();
        let result = ffi::deck_recommend_recommend_with_default_timeout(
            handle,
            invalid_json.as_ptr(),
            10,
            &mut error,
        );
        assert!(result.is_null());
        assert!(take_c_string(error).unwrap().contains("Failed to parse"));

        error = std::ptr::null();
        let result = ffi::deck_recommend_recommend_with_context(
            handle,
            invalid_json.as_ptr(),
            region.as_ptr(),
            std::ptr::null(),
            10,
            &mut error,
        );
        assert!(result.is_null());
        assert!(take_c_string(error).unwrap().contains("Failed to parse"));

        error = std::ptr::null();
        let result = ffi::deck_recommend_calculate(handle, invalid_json.as_ptr(), &mut error);
        assert!(result.is_null());
        assert!(take_c_string(error).unwrap().contains("Failed to parse"));

        error = std::ptr::null();
        let result = ffi::deck_recommend_get_world_bloom_support_cards(
            handle,
            invalid_json.as_ptr(),
            &mut error,
        );
        assert!(result.is_null());
        assert!(take_c_string(error).unwrap().contains("Failed to parse"));

        ffi::deck_recommend_destroy(handle);
        ffi::deck_recommend_destroy(std::ptr::null_mut());
    }
}
