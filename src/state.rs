use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::{Condvar, Mutex, MutexGuard};

use crate::bridge::DeckRecommend;

#[derive(Clone, Copy, Debug)]
pub struct DebugConfig {
    pub lock_warn_threshold: Duration,
    pub lock_timeout: Duration,
    pub engine_warn_threshold: Duration,
    pub default_recommend_timeout_ms: Option<i32>,
}

pub struct AppState {
    pub engines: EnginePool,
    pub next_op_id: AtomicU64,
    pub debug: DebugConfig,
    pub userdata_cache: UserdataCache,
}

impl AppState {
    pub fn next_op_id(&self) -> u64 {
        self.next_op_id.fetch_add(1, Ordering::Relaxed) + 1
    }
}

pub struct EnginePool {
    state: Mutex<EnginePoolState>,
    condvar: Condvar,
    size: usize,
}

struct EngineSlot {
    engine: DeckRecommend,
    userdata_hashes: HashSet<String>,
}

struct EnginePoolState {
    available: Vec<EngineSlot>,
    active_readers: usize,
    writer_active: bool,
    pending_writers: usize,
}

#[derive(Default)]
pub struct UserdataCache {
    entries: Mutex<HashMap<String, Arc<str>>>,
}

impl UserdataCache {
    pub fn remember(&self, hash: &str, userdata: &str) {
        let hash = hash.trim();
        if hash.is_empty() {
            return;
        }
        self.entries
            .lock()
            .insert(hash.to_string(), Arc::<str>::from(userdata.to_string()));
    }

    pub fn get(&self, hash: &str) -> Option<Arc<str>> {
        self.entries.lock().get(hash.trim()).cloned()
    }

    pub fn clear(&self) {
        self.entries.lock().clear();
    }
}

#[derive(Clone, Copy, Debug)]
pub enum EnginePoolError {
    CheckoutTimeout(Duration),
    ExclusiveTimeout(Duration),
}

impl EnginePoolError {
    pub fn timeout_message(self) -> String {
        match self {
            EnginePoolError::CheckoutTimeout(timeout) => {
                format!("engine checkout timeout after {} ms", timeout.as_millis())
            }
            EnginePoolError::ExclusiveTimeout(timeout) => {
                format!("engine exclusive lock timeout after {} ms", timeout.as_millis())
            }
        }
    }
}

pub struct EngineLease<'a> {
    pool: &'a EnginePool,
    slot: Option<EngineSlot>,
}

impl EnginePool {
    pub fn new(size: usize) -> Result<Self, String> {
        let size = size.max(1);
        let mut available = Vec::with_capacity(size);
        for _ in 0..size {
            available.push(EngineSlot {
                engine: DeckRecommend::new()?,
                userdata_hashes: HashSet::new(),
            });
        }

        Ok(Self {
            state: Mutex::new(EnginePoolState {
                available,
                active_readers: 0,
                writer_active: false,
                pending_writers: 0,
            }),
            condvar: Condvar::new(),
            size,
        })
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn checkout(&self, timeout: Duration) -> Result<EngineLease<'_>, EnginePoolError> {
        let mut state = self.state.lock();
        let wait_result = self.condvar.wait_while_for(
            &mut state,
            |state| state.writer_active || state.pending_writers > 0 || state.available.is_empty(),
            timeout,
        );
        if wait_result.timed_out()
            && (state.writer_active || state.pending_writers > 0 || state.available.is_empty())
        {
            return Err(EnginePoolError::CheckoutTimeout(timeout));
        }

        state.active_readers += 1;
        let slot = state
            .available
            .pop()
            .expect("engine pool signaled availability without an engine");
        drop(state);

        Ok(EngineLease {
            pool: self,
            slot: Some(slot),
        })
    }

    pub fn checkout_all(
        &self,
        timeout: Duration,
    ) -> Result<ExclusiveEngineLease<'_>, EnginePoolError> {
        let mut state = self.state.lock();
        state.pending_writers += 1;

        let wait_result = self
            .condvar
            .wait_while_for(&mut state, |state| state.writer_active || state.active_readers > 0, timeout);
        if wait_result.timed_out() && (state.writer_active || state.active_readers > 0) {
            state.pending_writers -= 1;
            return Err(EnginePoolError::ExclusiveTimeout(timeout));
        }

        state.pending_writers -= 1;
        state.writer_active = true;
        debug_assert_eq!(state.available.len(), self.size);

        Ok(ExclusiveEngineLease { pool: self, state })
    }
}

impl std::ops::Deref for EngineLease<'_> {
    type Target = DeckRecommend;

    fn deref(&self) -> &Self::Target {
        &self
            .slot
            .as_ref()
            .expect("engine lease accessed after release")
            .engine
    }
}

impl EngineLease<'_> {
    pub fn has_userdata_hash(&self, hash: &str) -> bool {
        let hash = hash.trim();
        !hash.is_empty()
            && self
                .slot
                .as_ref()
                .expect("engine lease accessed after release")
                .userdata_hashes
                .contains(hash)
    }

    pub fn remember_userdata_hash(&mut self, hash: &str) {
        let hash = hash.trim();
        if hash.is_empty() {
            return;
        }
        self.slot
            .as_mut()
            .expect("engine lease accessed after release")
            .userdata_hashes
            .insert(hash.to_string());
    }

    pub fn forget_userdata_hash(&mut self, hash: &str) {
        let hash = hash.trim();
        if hash.is_empty() {
            return;
        }
        self.slot
            .as_mut()
            .expect("engine lease accessed after release")
            .userdata_hashes
            .remove(hash);
    }
}

impl Drop for EngineLease<'_> {
    fn drop(&mut self) {
        let mut state = self.pool.state.lock();
        if let Some(slot) = self.slot.take() {
            state.available.push(slot);
            state.active_readers = state.active_readers.saturating_sub(1);
        }
        drop(state);
        self.pool.condvar.notify_all();
    }
}

pub struct ExclusiveEngineLease<'a> {
    pool: &'a EnginePool,
    state: MutexGuard<'a, EnginePoolState>,
}

impl ExclusiveEngineLease<'_> {
    pub fn len(&self) -> usize {
        self.state.available.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &DeckRecommend> {
        self.state.available.iter().map(|slot| &slot.engine)
    }

    pub fn clear_userdata_hashes(&mut self) {
        for slot in &mut self.state.available {
            slot.userdata_hashes.clear();
        }
    }
}

impl Drop for ExclusiveEngineLease<'_> {
    fn drop(&mut self) {
        self.state.writer_active = false;
        self.pool.condvar.notify_all();
    }
}
