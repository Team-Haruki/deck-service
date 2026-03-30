use std::sync::Mutex;

use crate::bridge::DeckRecommend;

pub struct AppState {
    pub engine: Mutex<DeckRecommend>,
}
