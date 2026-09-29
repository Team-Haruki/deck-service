//! JP 7.0.0 engine rules through the service bridge: MySekai gate 6 (no level
//! rows) and area item `multi_unit` effects with `multi_unit_bonus_evaluation`.
//!
//! The synthetic tests mirror the engine's own binding tests: every card has a
//! base power of 30000 per stat, so an area item effect of r% adds 900 * r and a
//! gate of level L adds 90000 * L / 1000 to a card.
//!
//! `jp_master_calculations_accept_gate_6_and_multi_unit_evaluation` runs only
//! when `DECK_TEST_JP_MASTER_DIR` points at a JP 7.0.0+ master data directory.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use deck_service::bridge::DeckRecommend;
use sonic_rs::{JsonContainerTrait, JsonValueTrait, Value, json};

const MASTER_DATA_KEYS: &[&str] = &[
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

const UNITS: [&str; 5] = [
    "light_sound",
    "idol",
    "street",
    "theme_park",
    "school_refusal",
];

// card id -> (character id, support unit)
const CARDS: &[(i32, i32, &str)] = &[
    (1, 1, "none"),
    (2, 2, "none"),
    (5, 5, "none"),
    (6, 6, "none"),
    (9, 9, "none"),
    (21, 21, "none"),
    (22, 22, "none"),
    (23, 23, "none"),
    (24, 24, "none"),
    (25, 25, "none"),
    (122, 22, "idol"),
];

const MULTI_ITEM: i32 = 56; // level 1: all characters 1% + multi_unit 3%
const MIXED_DECK: [i32; 5] = [1, 2, 5, 6, 21];
const SINGLE_VS_DECK: [i32; 5] = [21, 22, 23, 24, 25];
const GATE_DECK: [i32; 5] = [1, 5, 9, 21, 122];

fn character_unit(character_id: i32) -> &'static str {
    if character_id > 20 {
        "piapro"
    } else {
        UNITS[((character_id - 1) / 4) as usize]
    }
}

fn to_json(value: &Value) -> String {
    sonic_rs::to_string(value).unwrap()
}

fn area_item_level(level: i32, rate: f64, unit: &str) -> Value {
    json!({
        "areaItemId": MULTI_ITEM,
        "level": level,
        "targetUnit": unit,
        "targetCardAttr": "any",
        "targetGameCharacterId": 0,
        "power1BonusRate": rate,
        "power2BonusRate": rate,
        "power3BonusRate": rate,
        "power1AllMatchBonusRate": 0.0,
        "power2AllMatchBonusRate": 0.0,
        "power3AllMatchBonusRate": 0.0
    })
}

fn synthetic_masterdata() -> HashMap<String, String> {
    let mut data = MASTER_DATA_KEYS
        .iter()
        .map(|key| ((*key).to_owned(), "[]".to_owned()))
        .collect::<HashMap<_, _>>();
    let mut set = |key: &str, value: Value| {
        data.insert(key.to_owned(), to_json(&value));
    };

    let cards: Vec<Value> = CARDS
        .iter()
        .map(|&(id, character_id, support_unit)| {
            json!({
                "id": id,
                "characterId": character_id,
                "cardRarityType": "rarity_4",
                "attr": "cute",
                "supportUnit": support_unit,
                "skillId": 1,
                "cardParameters": [
                    {"cardLevel": 1, "cardParameterType": "param1", "power": 30000},
                    {"cardLevel": 1, "cardParameterType": "param2", "power": 30000},
                    {"cardLevel": 1, "cardParameterType": "param3", "power": 30000}
                ]
            })
        })
        .collect();
    set("cards", json!(cards));
    set(
        "cardRarities",
        json!([{"cardRarityType": "rarity_4", "maxLevel": 1, "trainingMaxLevel": 1, "maxSkillLevel": 1}]),
    );
    let characters: Vec<i32> = (1..=26).collect();
    set(
        "characterRanks",
        json!(
            characters
                .iter()
                .map(|c| json!({"id": c, "characterId": c, "characterRank": 1}))
                .collect::<Vec<_>>()
        ),
    );
    set(
        "gameCharacters",
        json!(
            characters
                .iter()
                .map(|&c| json!({"id": c, "unit": character_unit(c)}))
                .collect::<Vec<_>>()
        ),
    );
    set(
        "gameCharacterUnits",
        json!(
            characters
                .iter()
                .map(|&c| json!({"id": c, "gameCharacterId": c, "unit": character_unit(c)}))
                .collect::<Vec<_>>()
        ),
    );
    set(
        "skills",
        json!([{
            "id": 1,
            "skillEffects": [{
                "id": 1,
                "skillEffectType": "score_up",
                "skillEffectDetails": [{"id": 1, "level": 1, "activateEffectValue": 100}]
            }]
        }]),
    );
    set(
        "worldBloomDifferentAttributeBonuses",
        json!(
            (1..=5)
                .map(|count| json!({"attributeCount": count, "bonusRate": 0}))
                .collect::<Vec<_>>()
        ),
    );

    set(
        "areas",
        json!([{"id": 27, "areaType": "reality_world", "viewType": "side_view"}]),
    );
    set("areaItems", json!([{"id": MULTI_ITEM, "areaId": 27}]));
    set(
        "areaItemLevels",
        json!([
            area_item_level(1, 1.0, "any"),
            area_item_level(1, 3.0, "multi_unit")
        ]),
    );

    // JP 7.0.0 gates: five unit gates with level rows, gate 6 ("shuffle",
    // unit "none") has none.
    let mut gates: Vec<Value> = UNITS
        .iter()
        .enumerate()
        .map(|(index, unit)| json!({"id": index + 1, "unit": unit, "mysekaiGateType": "unit"}))
        .collect();
    gates.push(json!({"id": 6, "unit": "none", "mysekaiGateType": "shuffle"}));
    set("mysekaiGates", json!(gates));
    let mut gate_levels = Vec::new();
    for gate_id in 1..=5 {
        for level in 1..=70 {
            gate_levels.push(json!({
                "id": gate_id * 1000 + level,
                "mysekaiGateId": gate_id,
                "level": level,
                "powerBonusRate": f64::from(level) / 10.0
            }));
        }
    }
    set("mysekaiGateLevels", json!(gate_levels));
    data
}

const MUSIC_METAS: &str = r#"[{"music_id":1,"difficulty":"expert","music_time":100,"event_rate":100,"base_score":1,"base_score_auto":1,"skill_score_solo":[0,0,0,0,0,0],"skill_score_auto":[0,0,0,0,0,0],"skill_score_multi":[0,0,0,0,0,0],"fever_score":0,"fever_end_time":0,"tap_count":100}]"#;

/// `area_item_level` of item 56 (0 = not owned), gates as (gate id, level) and
/// user deck 1 made of `deck`.
fn synthetic_userdata(area_item_level: i32, gates: &[(i32, i32)], deck: &[i32]) -> String {
    let area_items = if area_item_level > 0 {
        json!([{"areaItemId": MULTI_ITEM, "level": area_item_level}])
    } else {
        json!([])
    };
    let user_cards: Vec<Value> = CARDS
        .iter()
        .map(|&(card_id, _, _)| {
            json!({
                "userId": 1,
                "cardId": card_id,
                "level": 1,
                "skillLevel": 1,
                "masterRank": 0,
                "specialTrainingStatus": "not_doing",
                "defaultImage": "original",
                "episodes": []
            })
        })
        .collect();
    let user_gates: Vec<Value> = gates
        .iter()
        .map(|&(gate_id, level)| json!({"mysekaiGateId": gate_id, "mysekaiGateLevel": level}))
        .collect();
    to_json(&json!({
        "userGamedata": {"userId": 1},
        "userAreas": [{"areaId": 27, "areaItems": area_items}],
        "userCards": user_cards,
        "userCharacters": (1..=26).map(|c| json!({"characterId": c, "characterRank": 1})).collect::<Vec<_>>(),
        "userChallengeLiveSoloDecks": [],
        "userDecks": [{
            "userId": 1,
            "deckId": 1,
            "leader": deck[0],
            "subLeader": deck[1],
            "member1": deck[0],
            "member2": deck[1],
            "member3": deck[2],
            "member4": deck[3],
            "member5": deck[4]
        }],
        "userHonors": [],
        "userMysekaiCanvases": [],
        "userMysekaiFixtureGameCharacterPerformanceBonuses": [],
        "userMysekaiGates": user_gates
    }))
}

/// The engine registers enum strings it has not seen before (`mapEnum`) in an
/// unsynchronised process-wide map, so first-time parses of master or user
/// data must not overlap. Tests in this binary run one at a time.
fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn init_static_data() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        DeckRecommend::init_data_path(concat!(env!("DECK_CPP_SRC_DIR"), "/data")).unwrap();
    });
}

/// The region data store is process-wide; the synthetic JP data is loaded once.
fn synthetic_engine() -> DeckRecommend {
    static LOADED: OnceLock<()> = OnceLock::new();
    init_static_data();
    let engine = DeckRecommend::new().unwrap();
    LOADED.get_or_init(|| {
        engine
            .update_masterdata_from_json(&synthetic_masterdata(), "jp")
            .unwrap();
        engine
            .update_musicmetas_from_string(MUSIC_METAS, "jp")
            .unwrap();
    });
    engine
}

fn i64_field(value: &Value, key: &str) -> i64 {
    value
        .get(key)
        .and_then(|v| v.as_i64())
        .unwrap_or_else(|| panic!("missing integer field {key} in {}", to_json(value)))
}

fn recommend_power_deck(
    engine: &DeckRecommend,
    userdata: &str,
    cards: &[i32],
    evaluation: Option<&str>,
) -> Result<Value, String> {
    let mut options = json!({
        "region": "jp",
        "user_data_str": userdata,
        "live_type": "multi",
        "music_id": 1,
        "music_diff": "expert",
        "target": "power",
        "algorithm": "dfs",
        "limit": 1,
        "member": cards.len(),
        "fixed_cards": cards
    });
    if let Some(evaluation) = evaluation {
        options.insert("multi_unit_bonus_evaluation", json!(evaluation));
    }
    let result: Value = sonic_rs::from_str(&engine.recommend_raw(&to_json(&options))?).unwrap();
    Ok(result
        .get("decks")
        .and_then(|decks| decks.as_array())
        .and_then(|decks| decks.first())
        .cloned()
        .expect("recommend returned no deck"))
}

/// Per-card `field` of a recommended deck, keyed by card id.
fn recommend_card_values(deck: &Value, field: &str) -> HashMap<i64, i64> {
    deck.get("cards")
        .and_then(|cards| cards.as_array())
        .unwrap()
        .iter()
        .map(|card| (i64_field(card, "card_id"), i64_field(card, field)))
        .collect()
}

fn calculate_deck(
    engine: &DeckRecommend,
    userdata: &str,
    evaluation: Option<&str>,
) -> Result<Value, String> {
    let mut options = json!({
        "region": "jp",
        "mode": "deck",
        "deck_id": 1,
        "user_data_str": userdata
    });
    if let Some(evaluation) = evaluation {
        options.insert("multi_unit_bonus_evaluation", json!(evaluation));
    }
    Ok(sonic_rs::from_str(&engine.calculate_raw(&to_json(&options))?).unwrap())
}

/// Per-card `field` of a /calculate deck result, keyed by card id.
fn calculate_card_values(result: &Value, field: &str) -> HashMap<i64, i64> {
    result
        .get("detail")
        .and_then(|detail| detail.get("cards"))
        .and_then(|cards| cards.as_array())
        .unwrap()
        .iter()
        .map(|card| {
            (
                i64_field(card, "cardId"),
                i64_field(card.get("power").unwrap(), field),
            )
        })
        .collect()
}

fn uniform(values: &HashMap<i64, i64>) -> i64 {
    let mut distinct: Vec<i64> = values.values().copied().collect();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), 1, "expected one value, got {values:?}");
    distinct[0]
}

#[test]
fn recommend_applies_multi_unit_bonus_evaluation() {
    let _serial = serial();
    let engine = synthetic_engine();
    let userdata = synthetic_userdata(1, &[], &MIXED_DECK);
    let area_bonus = |cards: &[i32], evaluation: Option<&str>| {
        let deck = recommend_power_deck(&engine, &userdata, cards, evaluation).unwrap();
        uniform(&recommend_card_values(&deck, "area_item_bonus_power"))
    };

    // Mixed units: by_deck (default and explicit) applies the 3% multi_unit row.
    assert_eq!(area_bonus(&MIXED_DECK, None), 900 * 4);
    assert_eq!(area_bonus(&MIXED_DECK, Some("by_deck")), 900 * 4);
    assert_eq!(area_bonus(&MIXED_DECK, Some("force_off")), 900);
    // Five unsupported virtual singers are one unit: only force_on adds it.
    assert_eq!(area_bonus(&SINGLE_VS_DECK, None), 900);
    assert_eq!(area_bonus(&SINGLE_VS_DECK, Some("force_on")), 900 * 4);

    let error =
        recommend_power_deck(&engine, &userdata, &MIXED_DECK, Some("sometimes")).unwrap_err();
    assert!(
        error.contains("Invalid multi unit bonus evaluation"),
        "{error}"
    );
}

#[test]
fn calculate_applies_multi_unit_bonus_evaluation() {
    let _serial = serial();
    let engine = synthetic_engine();
    let area_bonus = |deck: &[i32], evaluation: Option<&str>| {
        let userdata = synthetic_userdata(1, &[], deck);
        let result = calculate_deck(&engine, &userdata, evaluation).unwrap();
        let bonuses = calculate_card_values(&result, "areaItemBonus");
        let total = result
            .get("detail")
            .and_then(|detail| detail.get("power"))
            .map(|power| i64_field(power, "areaItemBonus"))
            .unwrap();
        assert_eq!(total, bonuses.values().sum::<i64>());
        uniform(&bonuses)
    };

    assert_eq!(area_bonus(&MIXED_DECK, None), 900 * 4);
    assert_eq!(area_bonus(&MIXED_DECK, Some("force_off")), 900);
    assert_eq!(area_bonus(&SINGLE_VS_DECK, Some("by_deck")), 900);
    assert_eq!(area_bonus(&SINGLE_VS_DECK, Some("force_on")), 900 * 4);
}

#[test]
fn gate_6_without_level_rows_does_not_break_calculations() {
    let _serial = serial();
    let engine = synthetic_engine();
    // Base power is 90000 per card; level 70 = 7%, level 40 = 4%.
    let expected = HashMap::from([
        (1, 6300),   // light_sound gate, level 70
        (5, 3600),   // idol gate, level 40
        (9, 0),      // no street gate
        (21, 6300),  // unsupported virtual singer: highest-level gate
        (122, 3600), // virtual singer supporting idol: idol gate
    ]);
    let userdata = synthetic_userdata(0, &[(1, 70), (2, 40), (6, 1)], &GATE_DECK);

    let deck = recommend_power_deck(&engine, &userdata, &GATE_DECK, None).unwrap();
    assert_eq!(recommend_card_values(&deck, "gate_bonus_power"), expected);
    assert_eq!(
        i64_field(&deck, "gate_bonus_power"),
        expected.values().sum::<i64>()
    );

    let result = calculate_deck(&engine, &userdata, None).unwrap();
    assert_eq!(calculate_card_values(&result, "gateBonus"), expected);

    // Gate 6 as the virtual singer's highest-level gate contributes 0 rather
    // than failing or falling back to another gate.
    let userdata = synthetic_userdata(0, &[(1, 70), (2, 40), (6, 71)], &GATE_DECK);
    let result = calculate_deck(&engine, &userdata, None).unwrap();
    let bonuses = calculate_card_values(&result, "gateBonus");
    assert_eq!(bonuses[&21], 0);
    assert_eq!(bonuses[&1], 6300);
}

// ---- real JP master data (opt-in) ----

fn read_json(dir: &std::path::Path, key: &str) -> Value {
    let path = dir.join(format!("{key}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
    sonic_rs::from_str(&text).unwrap()
}

/// First rarity_4 card without a support unit for each character, in order.
fn pick_cards(cards: &Value, characters: &[i64]) -> Vec<i64> {
    let cards = cards.as_array().unwrap();
    characters
        .iter()
        .map(|&character_id| {
            cards
                .iter()
                .find(|card| {
                    card.get("characterId").and_then(|v| v.as_i64()) == Some(character_id)
                        && card.get("cardRarityType").and_then(|v| v.as_str()) == Some("rarity_4")
                        && card.get("supportUnit").and_then(|v| v.as_str()) == Some("none")
                })
                .and_then(|card| card.get("id").and_then(|v| v.as_i64()))
                .unwrap_or_else(|| panic!("no rarity_4 card for character {character_id}"))
        })
        .collect()
}

fn real_userdata(area_item_56_area: i64, deck: &[i64], gates: &[(i64, i64)]) -> String {
    let user_cards: Vec<Value> = deck
        .iter()
        .map(|&card_id| {
            json!({
                "userId": 1,
                "cardId": card_id,
                "level": 60,
                "skillLevel": 1,
                "masterRank": 0,
                "specialTrainingStatus": "done",
                "defaultImage": "special_training",
                "episodes": []
            })
        })
        .collect();
    let user_gates: Vec<Value> = gates
        .iter()
        .map(|&(gate_id, level)| json!({"mysekaiGateId": gate_id, "mysekaiGateLevel": level}))
        .collect();
    to_json(&json!({
        "userGamedata": {"userId": 1},
        "userAreas": [{
            "areaId": area_item_56_area,
            "areaItems": [{"areaItemId": 56, "level": 10}]
        }],
        "userCards": user_cards,
        "userCharacters": (1..=26).map(|c| json!({"characterId": c, "characterRank": 1})).collect::<Vec<_>>(),
        "userChallengeLiveSoloDecks": [],
        "userDecks": [{
            "userId": 1,
            "deckId": 1,
            "leader": deck[0],
            "subLeader": deck[1],
            "member1": deck[0],
            "member2": deck[1],
            "member3": deck[2],
            "member4": deck[3],
            "member5": deck[4]
        }],
        "userHonors": [],
        "userMysekaiCanvases": [],
        "userMysekaiFixtureGameCharacterPerformanceBonuses": [],
        "userMysekaiGates": user_gates
    }))
}

#[test]
fn jp_master_calculations_accept_gate_6_and_multi_unit_evaluation() {
    let _serial = serial();
    let Some(dir) = std::env::var_os("DECK_TEST_JP_MASTER_DIR") else {
        eprintln!("DECK_TEST_JP_MASTER_DIR not set; skipping real JP master data test");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    init_static_data();

    // The engine core has no region-specific rules; a separate region keeps the
    // real data apart from the synthetic JP data loaded by the other tests.
    let region = "kr";
    let engine = DeckRecommend::new().unwrap();
    engine
        .update_masterdata(dir.to_str().unwrap(), region)
        .unwrap();
    engine
        .update_musicmetas_from_string(MUSIC_METAS, region)
        .unwrap();

    let area_items = read_json(&dir, "areaItems");
    let area_id = area_items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item.get("id").and_then(|v| v.as_i64()) == Some(56))
        .and_then(|item| item.get("areaId").and_then(|v| v.as_i64()))
        .expect("master data predates JP 7.0.0: area item 56 is missing");
    let gates = read_json(&dir, "mysekaiGates");
    assert!(
        gates
            .as_array()
            .unwrap()
            .iter()
            .any(|gate| gate.get("id").and_then(|v| v.as_i64()) == Some(6)),
        "master data predates JP 7.0.0: gate 6 is missing"
    );

    // light_sound, idol, street, theme_park, virtual singer: a multi-unit deck.
    let deck = pick_cards(&read_json(&dir, "cards"), &[1, 5, 9, 13, 21]);
    let userdata = real_userdata(area_id, &deck, &[(1, 40), (6, 1)]);

    let calculate = |evaluation: Option<&str>| {
        let mut options = json!({
            "region": region,
            "mode": "deck",
            "deck_id": 1,
            "user_data_str": userdata
        });
        if let Some(evaluation) = evaluation {
            options.insert("multi_unit_bonus_evaluation", json!(evaluation));
        }
        let result: Value =
            sonic_rs::from_str(&engine.calculate_raw(&to_json(&options)).unwrap()).unwrap();
        result
    };
    let by_deck = calculate(None);
    let forced_on = calculate(Some("force_on"));
    let forced_off = calculate(Some("force_off"));

    let area = |result: &Value| calculate_card_values(result, "areaItemBonus");
    let gate = calculate_card_values(&by_deck, "gateBonus");
    assert!(
        gate[&deck[0]] > 0,
        "light_sound gate should apply: {gate:?}"
    );
    assert_eq!(gate[&deck[1]], 0, "no idol gate owned: {gate:?}");
    assert_eq!(area(&by_deck), area(&forced_on));
    for card_id in &deck {
        assert!(
            area(&by_deck)[card_id] > area(&forced_off)[card_id],
            "multi_unit row should add area item bonus for card {card_id}"
        );
    }

    let mut options = json!({
        "region": region,
        "user_data_str": userdata,
        "live_type": "multi",
        "music_id": 1,
        "music_diff": "expert",
        "target": "power",
        "algorithm": "dfs",
        "limit": 1,
        "fixed_cards": deck
    });
    let by_deck_power = {
        let result: Value =
            sonic_rs::from_str(&engine.recommend_raw(&to_json(&options)).unwrap()).unwrap();
        i64_field(&result.get("decks").unwrap()[0], "area_item_bonus_power")
    };
    options.insert("multi_unit_bonus_evaluation", json!("force_off"));
    let forced_off_power = {
        let result: Value =
            sonic_rs::from_str(&engine.recommend_raw(&to_json(&options)).unwrap()).unwrap();
        i64_field(&result.get("decks").unwrap()[0], "area_item_bonus_power")
    };
    assert!(by_deck_power > forced_off_power);
}
