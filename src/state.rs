use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::{Condvar, Mutex, MutexGuard};

use crate::bridge::DeckRecommend;
use crate::registry::{RegionMasterState, RegistryClient};

#[derive(Clone, Copy, Debug)]
pub struct DebugConfig {
    pub lock_warn_threshold: Duration,
    pub lock_timeout: Duration,
    pub engine_warn_threshold: Duration,
    pub default_recommend_timeout_ms: Option<i32>,
    pub engine_thread_count: usize,
}

pub struct AppState {
    pub engines: EnginePool,
    pub next_op_id: AtomicU64,
    pub debug: DebugConfig,
    pub userdata_cache: UserdataCache,
    /// `Some` when `DECK_REGISTRY_URL` is set.
    pub registry: Option<Arc<RegistryClient>>,
    /// Regions loaded from the registry, keyed by lowercase region.
    /// Directory-loaded regions are not tracked here.
    pub masterdata_state: Mutex<HashMap<String, RegionMasterState>>,
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

pub const DEFAULT_USERDATA_CACHE_MAX: usize = 64;

/// How an exclusive engine update invalidates cached userdata.
#[derive(Clone, Copy, Debug)]
pub enum UserdataInvalidation<'a> {
    None,
    Region(&'a str),
    All,
}

struct UserdataEntry {
    payload: Arc<str>,
    /// Lowercase regions this entry has been used with; empty = never used.
    regions: Vec<String>,
}

#[derive(Default)]
struct UserdataCacheInner {
    entries: HashMap<String, UserdataEntry>,
    /// Front = least recently used.
    order: VecDeque<String>,
}

impl UserdataCacheInner {
    fn touch(&mut self, hash: &str) {
        self.order.retain(|h| h != hash);
        self.order.push_back(hash.to_string());
    }

    fn remove_all(&mut self, hashes: &[String]) {
        for hash in hashes {
            self.entries.remove(hash);
        }
        self.order.retain(|h| self.entries.contains_key(h));
    }
}

/// Bounded LRU of userdata payloads keyed by hash, tagged with the regions
/// they were used with so a region update can keep other regions' entries.
pub struct UserdataCache {
    inner: Mutex<UserdataCacheInner>,
    capacity: usize,
}

impl Default for UserdataCache {
    fn default() -> Self {
        Self::new(DEFAULT_USERDATA_CACHE_MAX)
    }
}

impl UserdataCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(UserdataCacheInner::default()),
            capacity: capacity.max(1),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.inner.lock().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Insert or replace; moves `hash` to MRU; evicts LRU entries while
    /// len > capacity. Returns the evicted hashes.
    pub fn remember(&self, hash: &str, userdata: &str) -> Vec<String> {
        let hash = hash.trim();
        if hash.is_empty() {
            return Vec::new();
        }
        let payload = Arc::<str>::from(userdata);
        let mut inner = self.inner.lock();
        match inner.entries.get_mut(hash) {
            Some(entry) => entry.payload = payload,
            None => {
                inner.entries.insert(
                    hash.to_string(),
                    UserdataEntry {
                        payload,
                        regions: Vec::new(),
                    },
                );
            }
        }
        inner.touch(hash);

        let mut evicted = Vec::new();
        while inner.entries.len() > self.capacity {
            let Some(oldest) = inner.order.pop_front() else {
                break;
            };
            inner.entries.remove(&oldest);
            evicted.push(oldest);
        }
        evicted
    }

    /// Payload lookup; bumps recency. `region = Some(r)` also tags the entry with `r`.
    pub fn get(&self, hash: &str, region: Option<&str>) -> Option<Arc<str>> {
        let hash = hash.trim();
        let mut inner = self.inner.lock();
        let entry = inner.entries.get_mut(hash)?;
        if let Some(region) = region.map(normalize_region)
            && !region.is_empty()
            && !entry.regions.contains(&region)
        {
            entry.regions.push(region);
        }
        let payload = Arc::clone(&entry.payload);
        inner.touch(hash);
        Some(payload)
    }

    pub fn clear(&self) -> Vec<String> {
        let mut inner = self.inner.lock();
        inner.order.clear();
        inner.entries.drain().map(|(hash, _)| hash).collect()
    }

    /// Evicts entries tagged with `region` plus untagged entries; keeps entries
    /// tagged only with other regions. Returns the evicted hashes.
    pub fn clear_region(&self, region: &str) -> Vec<String> {
        let region = normalize_region(region);
        let mut inner = self.inner.lock();
        let evicted: Vec<String> = inner
            .entries
            .iter()
            .filter(|(_, entry)| entry.regions.is_empty() || entry.regions.contains(&region))
            .map(|(hash, _)| hash.clone())
            .collect();
        inner.remove_all(&evicted);
        evicted
    }
}

fn normalize_region(region: &str) -> String {
    region.trim().to_ascii_lowercase()
}

/// Apply `how` to both the Rust payload cache and every engine slot's hash set.
/// Caller holds the exclusive lease (pool mutex); the cache mutex is taken inside.
/// Lock order is pool -> cache everywhere; nothing takes cache -> pool (readers
/// release the pool guard in `checkout` before touching the cache,
/// `cache_userdata` remembers after its lease is dropped), so this cannot deadlock.
pub fn invalidate_userdata(
    cache: &UserdataCache,
    engines: &mut ExclusiveEngineLease<'_>,
    how: UserdataInvalidation<'_>,
) -> usize {
    match how {
        UserdataInvalidation::None => 0,
        UserdataInvalidation::All => {
            let evicted = cache.clear().len();
            engines.clear_userdata_hashes();
            evicted
        }
        UserdataInvalidation::Region(region) => {
            let evicted = cache.clear_region(region);
            engines.forget_userdata_hashes(&evicted);
            evicted.len()
        }
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
                format!(
                    "engine exclusive lock timeout after {} ms",
                    timeout.as_millis()
                )
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

        let wait_result = self.condvar.wait_while_for(
            &mut state,
            |state| state.writer_active || state.active_readers > 0,
            timeout,
        );
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

    pub fn is_empty(&self) -> bool {
        self.state.available.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &DeckRecommend> {
        self.state.available.iter().map(|slot| &slot.engine)
    }

    pub fn clear_userdata_hashes(&mut self) {
        for slot in &mut self.state.available {
            slot.userdata_hashes.clear();
        }
    }

    pub fn forget_userdata_hashes(&mut self, hashes: &[String]) {
        if hashes.is_empty() {
            return;
        }
        for slot in &mut self.state.available {
            for hash in hashes {
                slot.userdata_hashes.remove(hash);
            }
        }
    }
}

impl Drop for ExclusiveEngineLease<'_> {
    fn drop(&mut self) {
        self.state.writer_active = false;
        self.pool.condvar.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn set(hashes: Vec<String>) -> HashSet<String> {
        hashes.into_iter().collect()
    }

    #[test]
    fn capacity_is_at_least_one() {
        assert_eq!(UserdataCache::new(0).capacity(), 1);
        assert_eq!(
            UserdataCache::default().capacity(),
            DEFAULT_USERDATA_CACHE_MAX
        );
        assert_eq!(DEFAULT_USERDATA_CACHE_MAX, 64);
        let cache = UserdataCache::new(3);
        assert!(cache.is_empty());
        assert!(cache.remember("A", "{}").is_empty());
        assert_eq!(cache.len(), 1);
        assert!(!cache.is_empty());
    }

    #[test]
    fn remember_evicts_least_recently_used() {
        let cache = UserdataCache::new(2);
        assert!(cache.remember("A", "a").is_empty());
        assert!(cache.remember("B", "b").is_empty());
        assert_eq!(cache.remember("C", "c"), vec!["A".to_string()]);
        assert!(cache.get("A", None).is_none());
        assert_eq!(cache.get("B", None).as_deref(), Some("b"));
        assert_eq!(cache.get("C", None).as_deref(), Some("c"));
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn get_bumps_recency() {
        let cache = UserdataCache::new(2);
        cache.remember("A", "a");
        cache.remember("B", "b");
        assert!(cache.get(" A ", None).is_some());
        assert_eq!(cache.remember("C", "c"), vec!["B".to_string()]);
        assert!(cache.get("A", None).is_some());
        assert!(cache.get("B", None).is_none());
    }

    #[test]
    fn remember_existing_key_moves_to_back_and_keeps_tags() {
        let cache = UserdataCache::new(2);
        cache.remember("A", "a1");
        assert!(cache.get("A", Some("jp")).is_some());
        cache.remember("B", "b");
        assert!(cache.remember("A", "a2").is_empty());
        assert_eq!(cache.remember("C", "c"), vec!["B".to_string()]);
        assert_eq!(cache.get("A", None).as_deref(), Some("a2"));

        // C is untagged and goes; A stays because its jp tag survived the replace.
        assert_eq!(cache.clear_region("cn"), vec!["C".to_string()]);
        assert!(cache.get("A", None).is_some());
        assert_eq!(cache.clear_region("jp"), vec!["A".to_string()]);
        assert!(cache.is_empty());
    }

    #[test]
    fn clear_region_evicts_tagged_and_untagged_only() {
        let cache = UserdataCache::new(8);
        cache.remember("A", "a");
        cache.remember("B", "b");
        cache.remember("C", "c");
        cache.get("A", Some("jp"));
        cache.get("B", Some("CN"));

        assert_eq!(
            set(cache.clear_region("JP ")),
            set(vec!["A".to_string(), "C".to_string()])
        );
        assert!(cache.get("B", None).is_some());
        assert!(cache.get("A", None).is_none());
        assert_eq!(cache.len(), 1);
        // The LRU order no longer references evicted hashes.
        cache.remember("D", "d");
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn clear_region_evicts_multi_region_entry() {
        let cache = UserdataCache::new(8);
        cache.remember("A", "a");
        cache.get("A", Some("jp"));
        cache.get("A", Some("cn"));
        cache.get("A", Some("jp"));
        cache.remember("B", "b");
        cache.get("B", Some("jp"));

        assert_eq!(cache.clear_region("cn"), vec!["A".to_string()]);
        assert!(cache.get("B", None).is_some());
    }

    #[test]
    fn clear_returns_everything() {
        let cache = UserdataCache::new(8);
        cache.remember("A", "a");
        cache.remember("B", "b");
        cache.get("B", Some("jp"));
        assert_eq!(
            set(cache.clear()),
            set(vec!["A".to_string(), "B".to_string()])
        );
        assert!(cache.is_empty());
        assert!(cache.clear().is_empty());
        assert!(cache.clear_region("jp").is_empty());
    }

    #[test]
    fn blank_hash_is_ignored() {
        let cache = UserdataCache::new(1);
        assert!(cache.remember("  ", "x").is_empty());
        assert!(cache.is_empty());
        assert!(cache.get("", Some("jp")).is_none());
        cache.remember(" A ", "a");
        assert_eq!(cache.get("A", Some(" ")).as_deref(), Some("a"));
        // A blank region is not a tag, so A is still untagged.
        assert_eq!(cache.clear_region("kr"), vec!["A".to_string()]);
    }
}
