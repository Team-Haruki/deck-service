use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

use crate::bridge::DeckRecommend;

#[derive(Clone, Copy, Debug)]
pub struct DebugConfig {
    pub lock_warn_threshold: Duration,
    pub lock_timeout: Duration,
    pub engine_warn_threshold: Duration,
    pub default_recommend_timeout_ms: Option<i32>,
}

pub struct AppState {
    pub engine: Mutex<DeckRecommend>,
    pub next_op_id: AtomicU64,
    pub debug: DebugConfig,
}

impl AppState {
    pub fn next_op_id(&self) -> u64 {
        self.next_op_id.fetch_add(1, Ordering::Relaxed) + 1
    }
}
