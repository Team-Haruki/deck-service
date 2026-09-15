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
    /// Lowercase regions this entry has been used with; empty = never used.
    regions: Vec<String>,
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
        // A replaced payload keeps the region tags of the entry it replaces.
        let regions = match entries.values.remove(hash) {
            Some(old) => {
                entries.bytes -= old.weight;
                old.regions
            }
            None => Vec::new(),
        };
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
                regions,
            },
        );
        entries.bytes += weight;
        Ok(())
    }

    /// Payload lookup; refreshes the idle timer. `region = Some(r)` also tags
    /// the entry with `r` so a later update of another region keeps it.
    pub fn get(&self, hash: &str, region: Option<&str>) -> Option<Arc<str>> {
        self.get_at(hash, region, Instant::now())
    }

    fn get_at(&self, hash: &str, region: Option<&str>, now: Instant) -> Option<Arc<str>> {
        let mut entries = self.entries.lock();
        self.expire_locked(&mut entries, now);
        let entry = entries.values.get_mut(hash.trim())?;
        entry.touched = now;
        if let Some(region) = region.map(normalize_region)
            && !region.is_empty()
            && !entry.regions.contains(&region)
        {
            entry.regions.push(region);
        }
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

    /// Drops every entry and returns the dropped hashes.
    pub fn clear(&self) -> Vec<String> {
        let mut entries = self.entries.lock();
        entries.bytes = 0;
        entries.values.drain().map(|(hash, _)| hash).collect()
    }

    /// Drops entries tagged with `region` plus untagged entries; keeps entries
    /// tagged only with other regions. Returns the dropped hashes.
    pub fn clear_region(&self, region: &str) -> Vec<String> {
        let region = normalize_region(region);
        let mut entries = self.entries.lock();
        let dropped: Vec<String> = entries
            .values
            .iter()
            .filter(|(_, entry)| entry.regions.is_empty() || entry.regions.contains(&region))
            .map(|(hash, _)| hash.clone())
            .collect();
        for hash in &dropped {
            if let Some(old) = entries.values.remove(hash) {
                entries.bytes -= old.weight;
            }
        }
        dropped
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

fn normalize_region(region: &str) -> String {
    region.trim().to_ascii_lowercase()
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
        let inflight = cache
            .get_at("a", None, now + Duration::from_secs(2))
            .unwrap();
        cache
            .remember_at("c", "third", now + Duration::from_secs(3))
            .unwrap();
        assert!(
            cache
                .get_at("b", None, now + Duration::from_secs(4))
                .is_none()
        );
        cache.clear();
        assert_eq!(&*inflight, "first");
        assert_eq!(cache.stats().bytes, 0);
    }

    #[test]
    fn byte_limit_replacement_and_oversized_rejection() {
        let cache = UserdataCache::new(600, 10, DEFAULT_TTL);
        cache.remember("a", &"a".repeat(200)).unwrap();
        cache.remember("b", &"b".repeat(200)).unwrap();
        assert!(cache.get("a", None).is_none());
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
        assert!(
            cache
                .get_at("a", None, now + Duration::from_secs(9))
                .is_some()
        );
        assert!(
            cache
                .get_at("a", None, now + Duration::from_secs(18))
                .is_some()
        );
        assert!(
            cache
                .get_at("a", None, now + Duration::from_secs(28))
                .is_none()
        );
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

    fn set(hashes: Vec<String>) -> std::collections::HashSet<String> {
        hashes.into_iter().collect()
    }

    #[test]
    fn clear_region_drops_tagged_and_untagged_only() {
        let cache = UserdataCache::default();
        cache.remember("A", "a").unwrap();
        cache.remember("B", "b").unwrap();
        cache.remember("C", "c").unwrap();
        cache.get("A", Some("jp"));
        cache.get("B", Some("CN"));

        assert_eq!(
            set(cache.clear_region("JP ")),
            set(vec!["A".to_string(), "C".to_string()])
        );
        assert!(cache.get("B", None).is_some());
        assert!(cache.get("A", None).is_none());
        let stats = cache.stats();
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.bytes, "b".len() + "B".len() + 128);
    }

    #[test]
    fn clear_region_drops_multi_region_entry() {
        let cache = UserdataCache::default();
        cache.remember("A", "a").unwrap();
        cache.get("A", Some("jp"));
        cache.get("A", Some("cn"));
        cache.get("A", Some("jp"));
        cache.remember("B", "b").unwrap();
        cache.get("B", Some("jp"));

        assert_eq!(cache.clear_region("cn"), vec!["A".to_string()]);
        assert!(cache.get("B", None).is_some());
    }

    #[test]
    fn replacing_a_payload_keeps_region_tags() {
        let cache = UserdataCache::default();
        cache.remember("A", "a1").unwrap();
        assert!(cache.get("A", Some("jp")).is_some());
        cache.remember("A", "a2").unwrap();
        cache.remember("C", "c").unwrap();

        // C is untagged and goes; A stays because its jp tag survived the replace.
        assert_eq!(cache.clear_region("cn"), vec!["C".to_string()]);
        assert_eq!(cache.get("A", None).as_deref(), Some("a2"));
        assert_eq!(cache.clear_region("jp"), vec!["A".to_string()]);
        assert_eq!(cache.stats().bytes, 0);
    }

    #[test]
    fn clear_returns_every_hash_and_blank_region_is_not_a_tag() {
        let cache = UserdataCache::default();
        cache.remember("A", "a").unwrap();
        assert!(cache.get("A", Some(" ")).is_some());
        assert_eq!(cache.clear_region("kr"), vec!["A".to_string()]);
        cache.remember("A", "a").unwrap();
        cache.remember("B", "b").unwrap();
        cache.get("B", Some("jp"));
        assert_eq!(
            set(cache.clear()),
            set(vec!["A".to_string(), "B".to_string()])
        );
        assert!(cache.clear().is_empty());
        assert!(cache.clear_region("jp").is_empty());
        assert_eq!(cache.stats().bytes, 0);
    }
}
