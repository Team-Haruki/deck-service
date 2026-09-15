use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;

pub const DEFAULT_MAX_BYTES: usize = 256 * 1024 * 1024;
pub const DEFAULT_MAX_ENTRIES: usize = 128;
pub const DEFAULT_TTL: Duration = Duration::from_secs(1800);
pub const MAX_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;

struct Entry {
    payload: Arc<str>,
    touched: Instant,
    weight: usize,
}

#[derive(Default)]
struct Entries {
    values: HashMap<String, Entry>,
    bytes: usize,
    evictions: u64,
}

pub struct UserdataCache {
    entries: Mutex<Entries>,
    max_bytes: usize,
    max_entries: usize,
    ttl: Duration,
}

#[derive(Serialize)]
pub struct CacheStats {
    pub entries: usize,
    pub bytes: usize,
    pub max_bytes: usize,
    pub max_entries: usize,
    pub ttl_seconds: u64,
    pub evictions: u64,
}

impl Default for UserdataCache {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_BYTES, DEFAULT_MAX_ENTRIES, DEFAULT_TTL)
    }
}

impl UserdataCache {
    pub fn new(max_bytes: usize, max_entries: usize, ttl: Duration) -> Self {
        assert!(max_bytes > 0 && max_entries > 0 && !ttl.is_zero());
        Self {
            entries: Mutex::default(),
            max_bytes,
            max_entries,
            ttl,
        }
    }

    pub fn validate_payload(&self, userdata: &str) -> Result<(), String> {
        // Reserve space for the hash and entry metadata as well as JSON bytes.
        if userdata.len() > MAX_PAYLOAD_BYTES || userdata.len().saturating_add(256) > self.max_bytes
        {
            return Err("userdata payload exceeds cache byte budget".into());
        }
        Ok(())
    }

    pub fn remember(&self, hash: &str, userdata: &str) -> Result<(), String> {
        self.remember_at(hash, userdata, Instant::now())
    }

    fn remember_at(&self, hash: &str, userdata: &str, now: Instant) -> Result<(), String> {
        self.validate_payload(userdata)?;
        let hash = hash.trim();
        if hash.is_empty() || hash.len() > 128 {
            return Err("invalid userdata hash".into());
        }
        let weight = userdata.len() + hash.len() + 128;
        let mut entries = self.entries.lock();
        self.expire_locked(&mut entries, now);
        if let Some(old) = entries.values.remove(hash) {
            entries.bytes -= old.weight;
        }
        while entries.values.len() >= self.max_entries || entries.bytes + weight > self.max_bytes {
            let oldest = entries
                .values
                .iter()
                .min_by_key(|(_, entry)| entry.touched)
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else {
                break;
            };
            let old = entries
                .values
                .remove(&oldest)
                .expect("selected cache entry exists");
            entries.bytes -= old.weight;
            entries.evictions += 1;
        }
        entries.values.insert(
            hash.to_owned(),
            Entry {
                payload: Arc::from(userdata),
                touched: now,
                weight,
            },
        );
        entries.bytes += weight;
        Ok(())
    }

    pub fn get(&self, hash: &str) -> Option<Arc<str>> {
        self.get_at(hash, Instant::now())
    }

    fn get_at(&self, hash: &str, now: Instant) -> Option<Arc<str>> {
        let mut entries = self.entries.lock();
        self.expire_locked(&mut entries, now);
        let entry = entries.values.get_mut(hash.trim())?;
        entry.touched = now;
        Some(Arc::clone(&entry.payload))
    }

    fn expire_locked(&self, entries: &mut Entries, now: Instant) {
        entries.values.retain(|_, entry| {
            if now.saturating_duration_since(entry.touched) < self.ttl {
                return true;
            }
            entries.bytes -= entry.weight;
            entries.evictions += 1;
            false
        });
    }

    pub fn expire(&self) {
        self.expire_locked(&mut self.entries.lock(), Instant::now());
    }

    pub fn clear(&self) {
        let mut entries = self.entries.lock();
        entries.values.clear();
        entries.bytes = 0;
    }

    pub fn stats(&self) -> CacheStats {
        let mut entries = self.entries.lock();
        self.expire_locked(&mut entries, Instant::now());
        CacheStats {
            entries: entries.values.len(),
            bytes: entries.bytes,
            max_bytes: self.max_bytes,
            max_entries: self.max_entries,
            ttl_seconds: self.ttl.as_secs(),
            evictions: entries.evictions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_limit_evicts_least_recently_used_and_keeps_inflight_payload() {
        let cache = UserdataCache::new(4096, 2, Duration::from_secs(30));
        let now = Instant::now();
        cache.remember_at("a", "first", now).unwrap();
        cache
            .remember_at("b", "second", now + Duration::from_secs(1))
            .unwrap();
        let inflight = cache.get_at("a", now + Duration::from_secs(2)).unwrap();
        cache
            .remember_at("c", "third", now + Duration::from_secs(3))
            .unwrap();
        assert!(cache.get_at("b", now + Duration::from_secs(4)).is_none());
        cache.clear();
        assert_eq!(&*inflight, "first");
        assert_eq!(cache.stats().bytes, 0);
    }

    #[test]
    fn byte_limit_replacement_and_oversized_rejection() {
        let cache = UserdataCache::new(600, 10, DEFAULT_TTL);
        cache.remember("a", &"a".repeat(200)).unwrap();
        cache.remember("b", &"b".repeat(200)).unwrap();
        assert!(cache.get("a").is_none());
        cache.remember("b", "x").unwrap();
        assert_eq!(cache.stats().bytes, 130);
        assert!(cache.remember("oversized", &"c".repeat(600)).is_err());
        assert_eq!(cache.stats().entries, 1);
        assert!(cache.stats().bytes <= 600);
    }

    #[test]
    fn idle_expiration_is_refreshed_on_read() {
        let cache = UserdataCache::new(4096, 8, Duration::from_secs(10));
        let now = Instant::now();
        cache.remember_at("a", "x", now).unwrap();
        assert!(cache.get_at("a", now + Duration::from_secs(9)).is_some());
        assert!(cache.get_at("a", now + Duration::from_secs(18)).is_some());
        assert!(cache.get_at("a", now + Duration::from_secs(28)).is_none());
        assert_eq!(cache.stats().bytes, 0);
    }

    #[test]
    fn concurrent_inserts_remain_bounded() {
        let cache = UserdataCache::new(8192, 8, DEFAULT_TTL);
        std::thread::scope(|scope| {
            for worker in 0..8 {
                let cache = &cache;
                scope.spawn(move || {
                    for i in 0..100 {
                        cache
                            .remember(&format!("{worker}-{i}"), &"x".repeat(512))
                            .unwrap();
                    }
                });
            }
        });
        let stats = cache.stats();
        assert!(stats.bytes <= 8192 && stats.entries <= 8);
        assert!(stats.evictions > 0);
    }
}
