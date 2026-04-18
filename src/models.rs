use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sonic_rs::Value;

// ---- Request types ----

#[derive(Debug, Serialize, Deserialize)]
pub struct CardConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level_max: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_read: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub master_max: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_max: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canvas: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SingleCardConfig {
    pub card_id: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level_max: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_read: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub master_max: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_max: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canvas: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_num: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_iter: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_no_improve_iter: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_limit_ms: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_temprature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooling_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_iter: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_no_improve_iter: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pop_size: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_size: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elite_size: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crossover_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_mutation_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_improve_iter_to_mutation_rate: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeckRecommendOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub algorithm: Option<String>,
    pub region: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub userdata_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_data_file_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_data_str: Option<String>,
    pub live_type: String,
    pub music_id: i32,
    pub music_diff: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_attr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub world_bloom_event_turn: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub world_bloom_character_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub challenge_live_character_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rarity_1_config: Option<CardConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rarity_2_config: Option<CardConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rarity_3_config: Option<CardConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rarity_birthday_config: Option<CardConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rarity_4_config: Option<CardConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub single_card_configs: Option<Vec<SingleCardConfig>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_other_unit: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed_cards: Option<Vec<i32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed_characters: Option<Vec<i32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_bonus_list: Option<Vec<i32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_reference_choose_strategy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_after_training_state: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multi_live_teammate_score_up: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multi_live_teammate_power: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_skill_as_leader: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multi_live_score_up_lower_bound: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_order_choose_strategy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub specific_skill_order: Option<Vec<i32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sa_options: Option<SaOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ga_options: Option<GaOptions>,
}

// ---- Response types ----

#[derive(Debug, Serialize, Deserialize)]
pub struct RecommendCard {
    pub card_id: i32,
    pub total_power: i32,
    pub base_power: i32,
    pub event_bonus_rate: f64,
    pub master_rank: i32,
    pub level: i32,
    pub skill_level: i32,
    pub skill_score_up: f64,
    pub skill_life_recovery: f64,
    pub episode1_read: bool,
    pub episode2_read: bool,
    pub after_training: bool,
    pub default_image: String,
    pub has_canvas_bonus: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RecommendSupportDeckCard {
    pub card_id: i32,
    pub bonus: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RecommendDeck {
    pub score: i32,
    pub live_score: i32,
    pub mysekai_event_point: i32,
    pub total_power: i32,
    pub base_power: i32,
    pub area_item_bonus_power: i32,
    pub character_bonus_power: i32,
    pub honor_bonus_power: i32,
    pub fixture_bonus_power: i32,
    pub gate_bonus_power: i32,
    pub event_bonus_rate: f64,
    pub support_deck_bonus_rate: f64,
    pub multi_live_score_up: f64,
    pub cards: Vec<RecommendCard>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub support_deck_cards: Option<Vec<RecommendSupportDeckCard>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeckRecommendResult {
    pub decks: Vec<RecommendDeck>,
}

pub type BatchRecommendOption = BTreeMap<String, Value>;

#[derive(Debug, Deserialize)]
pub struct BatchRecommendRequest {
    pub region: String,
    pub batch_options: Vec<BatchRecommendOption>,
    pub userdata_hash: String,
}

#[derive(Debug, Serialize)]
pub struct CacheUserdataResponse {
    pub userdata_hash: String,
}

#[derive(Debug, Serialize)]
pub struct BatchRecommendResponseItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alg: Option<String>,
    pub cost_time: f64,
    pub wait_time: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<DeckRecommendResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---- Admin request types ----

#[derive(Debug, Deserialize)]
pub struct UpdateMasterdataRequest {
    pub base_dir: String,
    pub region: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMasterdataFromJsonRequest {
    pub data: std::collections::HashMap<String, String>,
    pub region: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMusicmetasRequest {
    pub file_path: String,
    pub region: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMusicmetasFromStringRequest {
    pub data: String,
    pub region: String,
}
