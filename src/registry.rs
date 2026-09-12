//! Master registry client: deck-service pulls the master data the engine
//! needs straight from the Haruki master registry (`/v1/master/{region}/...`)
//! over plain HTTP instead of reading a mounted directory that some other
//! service has to keep fresh.

use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::error::AppError;
use crate::state::AppState;

/// Keys the C++ engine reads (`sekai-deck-recommend-cpp`
/// `src/data-provider/master-data.cpp`). A missing required key aborts the
/// load; a missing optional key is skipped.
pub const REQUIRED_MASTERDATA_KEYS: [&str; 25] = [
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
];

pub const OPTIONAL_MASTERDATA_KEYS: [&str; 12] = [
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

pub const DEFAULT_REGIONS: [&str; 5] = ["jp", "en", "cn", "tw", "kr"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryConfig {
    pub url: String,
    pub regions: Vec<String>,
    pub refresh: Duration,
    pub fetch_concurrency: usize,
    pub timeout: Duration,
}

impl RegistryConfig {
    /// `None` when `DECK_REGISTRY_URL` is unset or blank: the directory path
    /// keeps working exactly as before.
    pub fn from_env() -> Option<Self> {
        let url = env::var("DECK_REGISTRY_URL").ok()?;
        Self::from_values(
            &url,
            env::var("DECK_REGISTRY_REGIONS").ok().as_deref(),
            env::var("DECK_REGISTRY_REFRESH_MS").ok().as_deref(),
            env::var("DECK_REGISTRY_FETCH_CONCURRENCY").ok().as_deref(),
            env::var("DECK_REGISTRY_TIMEOUT_MS").ok().as_deref(),
        )
    }

    pub fn from_values(
        url: &str,
        regions: Option<&str>,
        refresh_ms: Option<&str>,
        fetch_concurrency: Option<&str>,
        timeout_ms: Option<&str>,
    ) -> Option<Self> {
        let url = url.trim().trim_end_matches('/');
        if url.is_empty() {
            return None;
        }
        let regions = regions
            .map(|raw| {
                raw.split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(str::to_ascii_lowercase)
                    .collect::<Vec<_>>()
            })
            .filter(|list| !list.is_empty())
            .unwrap_or_else(|| DEFAULT_REGIONS.iter().map(|r| (*r).to_owned()).collect());
        let parse_u64 = |raw: Option<&str>, default: u64| {
            raw.and_then(|raw| raw.trim().parse::<u64>().ok())
                .unwrap_or(default)
        };
        Some(Self {
            url: url.to_owned(),
            regions,
            refresh: Duration::from_millis(parse_u64(refresh_ms, 300_000)),
            fetch_concurrency: parse_u64(fetch_concurrency, 8).clamp(1, 64) as usize,
            timeout: Duration::from_millis(parse_u64(timeout_ms, 30_000).max(1_000)),
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    #[serde(default)]
    pub server: String,
    #[serde(default)]
    pub data_version: String,
    #[serde(default)]
    pub content_hash: String,
    #[serde(default)]
    pub git_commit: String,
    #[serde(default)]
    pub files: Vec<ManifestFile>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ManifestFile {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    pub sha256: String,
}

/// What is loaded for one region. `music_metas_etag` stays internal (it is
/// the conditional-request token, not a fact about the data).
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RegionMasterState {
    pub content_hash: String,
    pub git_commit: String,
    pub data_version: String,
    pub loaded_at: u64,
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub music_metas_digest: Option<String>,
    #[serde(skip)]
    pub music_metas_etag: Option<String>,
}

#[derive(Debug)]
pub enum RegistryError {
    NotConfigured,
    Http(String),
    Status { url: String, status: u16 },
    Decode(String),
    MissingRequired(Vec<String>),
    Engine(String),
    Timeout(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::NotConfigured => write!(f, "registry is not configured"),
            RegistryError::Http(msg) => write!(f, "registry request failed: {msg}"),
            RegistryError::Status { url, status } => {
                write!(f, "registry returned HTTP {status} for {url}")
            }
            RegistryError::Decode(msg) => write!(f, "registry response invalid: {msg}"),
            RegistryError::MissingRequired(keys) => write!(
                f,
                "registry manifest lacks required master data: {}",
                keys.join(",")
            ),
            RegistryError::Engine(msg) => write!(f, "engine rejected master data: {msg}"),
            RegistryError::Timeout(msg) => write!(f, "{msg}"),
        }
    }
}

impl From<RegistryError> for AppError {
    fn from(err: RegistryError) -> Self {
        match err {
            RegistryError::NotConfigured => AppError::ServiceUnavailable(err.to_string()),
            RegistryError::Http(_) | RegistryError::Status { .. } => {
                AppError::Upstream(err.to_string())
            }
            RegistryError::Decode(_) | RegistryError::MissingRequired(_) => {
                AppError::Upstream(err.to_string())
            }
            RegistryError::Engine(msg) => AppError::Engine(msg),
            RegistryError::Timeout(msg) => AppError::Timeout(msg),
        }
    }
}

pub enum Fetched {
    NotModified,
    Body {
        bytes: Vec<u8>,
        etag: Option<String>,
    },
}

pub struct RegistryClient {
    base: String,
    http: reqwest::Client,
    fetch_concurrency: usize,
}

impl RegistryClient {
    pub fn new(cfg: &RegistryConfig) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .build()
            .map_err(|err| format!("build registry http client: {err}"))?;
        Ok(Self {
            base: cfg.url.trim_end_matches('/').to_owned(),
            http,
            fetch_concurrency: cfg.fetch_concurrency.max(1),
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    async fn get(&self, url: &str, etag: Option<&str>) -> Result<Fetched, RegistryError> {
        let mut req = self.http.get(url);
        if let Some(etag) = etag.filter(|value| !value.is_empty()) {
            req = req.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        let resp = req
            .send()
            .await
            .map_err(|err| RegistryError::Http(format!("{url}: {err}")))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(Fetched::NotModified);
        }
        if !status.is_success() {
            return Err(RegistryError::Status {
                url: url.to_owned(),
                status: status.as_u16(),
            });
        }
        let etag = resp
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes = resp
            .bytes()
            .await
            .map_err(|err| RegistryError::Http(format!("{url}: {err}")))?;
        Ok(Fetched::Body {
            bytes: bytes.to_vec(),
            etag,
        })
    }

    pub async fn fetch_manifest(&self, region: &str) -> Result<Manifest, RegistryError> {
        let url = format!("{}/v1/master/{region}/current", self.base);
        match self.get(&url, None).await? {
            Fetched::NotModified => Err(RegistryError::Decode(format!(
                "{url}: unexpected 304 without a conditional request"
            ))),
            Fetched::Body { bytes, .. } => sonic_rs::from_slice::<Manifest>(&bytes)
                .map_err(|err| RegistryError::Decode(format!("{url}: {err}"))),
        }
    }

    pub async fn fetch_blob(&self, region: &str, sha256: &str) -> Result<String, RegistryError> {
        let url = format!("{}/v1/master/{region}/blob/{sha256}", self.base);
        match self.get(&url, None).await? {
            Fetched::NotModified => Err(RegistryError::Decode(format!("{url}: unexpected 304"))),
            Fetched::Body { bytes, .. } => String::from_utf8(bytes)
                .map_err(|err| RegistryError::Decode(format!("{url}: not UTF-8: {err}"))),
        }
    }

    pub async fn fetch_music_metas(
        &self,
        region: &str,
        etag: Option<&str>,
    ) -> Result<Fetched, RegistryError> {
        let url = format!("{}/v1/metas/{region}/music_metas.json", self.base);
        self.get(&url, etag).await
    }

    /// Every engine key present in the manifest, fetched by digest with
    /// bounded concurrency. Required keys absent from the manifest fail the
    /// whole load so the engine never sees a partial region.
    pub async fn fetch_masterdata(
        &self,
        region: &str,
        manifest: &Manifest,
    ) -> Result<HashMap<String, String>, RegistryError> {
        let by_name: HashMap<&str, &ManifestFile> = manifest
            .files
            .iter()
            .map(|file| (file.name.as_str(), file))
            .collect();
        let missing: Vec<String> = REQUIRED_MASTERDATA_KEYS
            .iter()
            .filter(|key| !by_name.contains_key(format!("{key}.json").as_str()))
            .map(|key| (*key).to_owned())
            .collect();
        if !missing.is_empty() {
            return Err(RegistryError::MissingRequired(missing));
        }

        let wanted: Vec<(String, String)> = REQUIRED_MASTERDATA_KEYS
            .iter()
            .chain(OPTIONAL_MASTERDATA_KEYS.iter())
            .filter_map(|key| {
                by_name
                    .get(format!("{key}.json").as_str())
                    .map(|file| ((*key).to_owned(), file.sha256.clone()))
            })
            .collect();

        let semaphore = Arc::new(Semaphore::new(self.fetch_concurrency));
        let mut tasks = JoinSet::new();
        for (key, sha256) in wanted {
            let permit = semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|err| RegistryError::Http(err.to_string()))?;
            let http = self.http.clone();
            let url = format!("{}/v1/master/{region}/blob/{sha256}", self.base);
            tasks.spawn(async move {
                let _permit = permit;
                let resp = http
                    .get(&url)
                    .send()
                    .await
                    .map_err(|err| RegistryError::Http(format!("{url}: {err}")))?;
                if !resp.status().is_success() {
                    return Err(RegistryError::Status {
                        url,
                        status: resp.status().as_u16(),
                    });
                }
                let bytes = resp
                    .bytes()
                    .await
                    .map_err(|err| RegistryError::Http(format!("{url}: {err}")))?;
                let text = String::from_utf8(bytes.to_vec())
                    .map_err(|err| RegistryError::Decode(format!("{url}: not UTF-8: {err}")))?;
                Ok::<_, RegistryError>((key, text))
            });
        }
        let mut data = HashMap::new();
        while let Some(joined) = tasks.join_next().await {
            let (key, text) = joined.map_err(|err| RegistryError::Http(err.to_string()))??;
            data.insert(key, text);
        }
        Ok(data)
    }
}

#[derive(Debug)]
pub struct EnsureOutcome {
    pub state: RegionMasterState,
    pub reloaded: bool,
}

/// Bring `region` up to the registry's current manifest. `expected_hash`
/// (from a caller that already knows the registry's `contentHash`) short-
/// circuits without a round trip when it matches what is loaded.
pub async fn ensure_region(
    state: &Arc<AppState>,
    region: &str,
    expected_hash: Option<&str>,
    reason: &'static str,
) -> Result<EnsureOutcome, RegistryError> {
    let region = region.trim().to_ascii_lowercase();
    if region.is_empty() {
        return Err(RegistryError::Decode("region is required".into()));
    }
    let client = state
        .registry
        .as_ref()
        .ok_or(RegistryError::NotConfigured)?;

    let loaded = state.masterdata_state.lock().get(&region).cloned();
    if let (Some(expected), Some(loaded)) = (
        expected_hash.map(str::trim).filter(|hash| !hash.is_empty()),
        loaded.as_ref(),
    ) && expected == loaded.content_hash
    {
        return Ok(EnsureOutcome {
            state: loaded.clone(),
            reloaded: false,
        });
    }

    let started = Instant::now();
    let manifest = client.fetch_manifest(&region).await?;
    if manifest.content_hash.is_empty() {
        return Err(RegistryError::Decode(format!(
            "manifest for {region} carries no contentHash"
        )));
    }
    if let Some(loaded) = loaded.as_ref()
        && loaded.content_hash == manifest.content_hash
    {
        // Master data unchanged; music metas move independently.
        let refreshed = refresh_music_metas(state, client, &region, loaded).await?;
        return Ok(EnsureOutcome {
            state: refreshed,
            reloaded: false,
        });
    }

    tracing::info!(
        region = %region,
        reason,
        content_hash = %manifest.content_hash,
        git_commit = %manifest.git_commit,
        data_version = %manifest.data_version,
        "Loading deck-service masterdata from the registry"
    );
    let data = client.fetch_masterdata(&region, &manifest).await?;
    let metas = client.fetch_music_metas(&region, None).await?;
    let (metas_body, metas_etag) = match metas {
        Fetched::Body { bytes, etag } => (Some(bytes), etag),
        Fetched::NotModified => (None, None),
    };
    let metas_text = metas_body
        .map(|bytes| {
            String::from_utf8(bytes)
                .map_err(|err| RegistryError::Decode(format!("music_metas not UTF-8: {err}")))
        })
        .transpose()?;
    let fetch_elapsed = started.elapsed();

    let region_for_engine = region.clone();
    let metas_for_engine = metas_text.clone();
    tokio::task::block_in_place(|| {
        apply_exclusive(state, &region_for_engine, |engine| {
            engine.update_masterdata_from_json(&data, &region_for_engine)?;
            if let Some(text) = metas_for_engine.as_deref() {
                engine.update_musicmetas_from_string(text, &region_for_engine)?;
            }
            Ok(())
        })
    })?;

    let next = RegionMasterState {
        content_hash: manifest.content_hash.clone(),
        git_commit: manifest.git_commit.clone(),
        data_version: manifest.data_version.clone(),
        loaded_at: unix_now(),
        source: "registry",
        music_metas_digest: metas_text.as_deref().map(digest_hex),
        music_metas_etag: metas_etag,
    };
    state
        .masterdata_state
        .lock()
        .insert(region.clone(), next.clone());
    tracing::info!(
        region = %region,
        reason,
        content_hash = %next.content_hash,
        file_count = data.len(),
        music_metas = metas_text.is_some(),
        fetch_ms = fetch_elapsed.as_secs_f64() * 1000.0,
        total_ms = started.elapsed().as_secs_f64() * 1000.0,
        "Loaded deck-service masterdata from the registry"
    );
    Ok(EnsureOutcome {
        state: next,
        reloaded: true,
    })
}

async fn refresh_music_metas(
    state: &Arc<AppState>,
    client: &RegistryClient,
    region: &str,
    loaded: &RegionMasterState,
) -> Result<RegionMasterState, RegistryError> {
    let fetched = client
        .fetch_music_metas(region, loaded.music_metas_etag.as_deref())
        .await?;
    let Fetched::Body { bytes, etag } = fetched else {
        return Ok(loaded.clone());
    };
    let text = String::from_utf8(bytes)
        .map_err(|err| RegistryError::Decode(format!("music_metas not UTF-8: {err}")))?;
    let digest = digest_hex(&text);
    if loaded.music_metas_digest.as_deref() == Some(digest.as_str()) {
        let mut same = loaded.clone();
        same.music_metas_etag = etag.or(same.music_metas_etag);
        state
            .masterdata_state
            .lock()
            .insert(region.to_owned(), same.clone());
        return Ok(same);
    }
    let region_owned = region.to_owned();
    tokio::task::block_in_place(|| {
        apply_exclusive(state, &region_owned, |engine| {
            engine.update_musicmetas_from_string(&text, &region_owned)
        })
    })?;
    let mut next = loaded.clone();
    next.music_metas_digest = Some(digest);
    next.music_metas_etag = etag;
    state
        .masterdata_state
        .lock()
        .insert(region.to_owned(), next.clone());
    tracing::info!(region = %region, "Refreshed deck-service music metas from the registry");
    Ok(next)
}

/// Same shape as the handlers' exclusive path: master data lives in the
/// engine's shared region store, so one engine applies it for the pool;
/// cached userdata is then invalid for every slot.
fn apply_exclusive<F>(state: &AppState, region: &str, f: F) -> Result<(), RegistryError>
where
    F: FnOnce(&crate::bridge::DeckRecommend) -> Result<(), String>,
{
    let mut engines = state
        .engines
        .checkout_all(state.debug.lock_timeout)
        .map_err(|err| RegistryError::Timeout(err.timeout_message()))?;
    let engine = engines
        .iter()
        .next()
        .ok_or_else(|| RegistryError::Engine("engine pool is empty".into()))?;
    f(engine).map_err(RegistryError::Engine)?;
    engines.clear_userdata_hashes();
    state.userdata_cache.clear();
    tracing::debug!(region = %region, "Cleared cached userdata after registry update");
    Ok(())
}

pub async fn preload(state: &Arc<AppState>, cfg: &RegistryConfig) {
    for region in &cfg.regions {
        match ensure_region(state, region, None, "preload").await {
            Ok(outcome) => tracing::info!(
                region = %region,
                content_hash = %outcome.state.content_hash,
                reloaded = outcome.reloaded,
                "Registry preload finished"
            ),
            Err(err) => tracing::error!(
                region = %region,
                error = %err,
                "Registry preload failed; the refresh loop will retry"
            ),
        }
    }
}

pub fn spawn_refresh_loop(state: Arc<AppState>, cfg: RegistryConfig) {
    if cfg.refresh.is_zero() {
        tracing::info!("Registry refresh loop disabled");
        return;
    }
    tracing::info!(
        refresh_ms = cfg.refresh.as_millis() as u64,
        regions = %cfg.regions.join(","),
        registry_url = %cfg.url,
        "Registry refresh loop started"
    );
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(cfg.refresh).await;
            for region in &cfg.regions {
                match ensure_region(&state, region, None, "refresh").await {
                    Ok(outcome) if outcome.reloaded => tracing::info!(
                        region = %region,
                        content_hash = %outcome.state.content_hash,
                        "Registry refresh reloaded masterdata"
                    ),
                    Ok(_) => {}
                    Err(err) => tracing::warn!(
                        region = %region,
                        error = %err,
                        "Registry refresh failed"
                    ),
                }
            }
        }
    });
}

pub fn digest_hex(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    use axum::Router;
    use axum::extract::{Path, State};
    use axum::http::{HeaderMap, StatusCode, header};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use parking_lot::Mutex;

    use super::*;
    use crate::bridge::DeckRecommend;
    use crate::state::{DebugConfig, EnginePool, UserdataCache};

    const MUSIC_METAS_V1: &str = r#"[{"music_id":1,"difficulty":"master","music_time":120.0}]"#;
    const MUSIC_METAS_V2: &str = r#"[{"music_id":2,"difficulty":"master","music_time":130.0}]"#;

    #[test]
    fn masterdata_key_list_is_frozen() {
        let all: Vec<&str> = REQUIRED_MASTERDATA_KEYS
            .iter()
            .chain(OPTIONAL_MASTERDATA_KEYS.iter())
            .copied()
            .collect();
        assert_eq!(REQUIRED_MASTERDATA_KEYS.len(), 25);
        assert_eq!(OPTIONAL_MASTERDATA_KEYS.len(), 12);
        assert_eq!(all.len(), 37);
        assert_eq!(
            all.iter().collect::<HashSet<_>>().len(),
            37,
            "duplicate key"
        );
        for key in ["cards", "musics", "events", "worldBloomSupportDeckBonuses"] {
            assert!(
                REQUIRED_MASTERDATA_KEYS.contains(&key),
                "{key} must be required"
            );
        }
        for key in ["ingameNotes", "mysekaiGateLevels"] {
            assert!(
                OPTIONAL_MASTERDATA_KEYS.contains(&key),
                "{key} must be optional"
            );
        }
        assert!(all.iter().all(|key| !key.ends_with(".json")));
    }

    #[test]
    fn config_parses_env_values_with_defaults() {
        assert!(RegistryConfig::from_values("  ", None, None, None, None).is_none());
        let cfg = RegistryConfig::from_values("http://reg:9998/", None, None, None, None).unwrap();
        assert_eq!(cfg.url, "http://reg:9998");
        assert_eq!(cfg.regions, DEFAULT_REGIONS);
        assert_eq!(cfg.refresh, Duration::from_millis(300_000));
        assert_eq!(cfg.fetch_concurrency, 8);
        assert_eq!(cfg.timeout, Duration::from_millis(30_000));
        let cfg = RegistryConfig::from_values(
            "http://reg:9998",
            Some(" JP, cn ,,"),
            Some("0"),
            Some("999"),
            Some("5"),
        )
        .unwrap();
        assert_eq!(cfg.regions, vec!["jp", "cn"]);
        assert!(cfg.refresh.is_zero());
        assert_eq!(cfg.fetch_concurrency, 64);
        assert_eq!(cfg.timeout, Duration::from_millis(1_000));
    }

    #[derive(Default)]
    struct FakeRegistry {
        manifests: BTreeMap<String, Manifest>,
        blobs: BTreeMap<String, String>,
        metas: BTreeMap<String, (String, String)>,
        manifest_requests: AtomicUsize,
        blob_requests: AtomicUsize,
        metas_requests: AtomicUsize,
    }

    type Shared = Arc<Mutex<FakeRegistry>>;

    fn sha(text: &str) -> String {
        digest_hex(text)
    }

    fn publish(registry: &Shared, region: &str, keys: &[&str], version: &str) -> String {
        let mut fake = registry.lock();
        let mut files = Vec::new();
        for key in keys {
            let body = "[]".to_string();
            let digest = sha(&format!("{version}:{key}:{body}"));
            fake.blobs.insert(digest.clone(), body);
            files.push(ManifestFile {
                name: format!("{key}.json"),
                size: 2,
                sha256: digest,
            });
        }
        let content_hash = sha(&format!("{region}:{version}:{}", keys.join(",")));
        fake.manifests.insert(
            region.to_owned(),
            Manifest {
                server: region.to_owned(),
                data_version: version.to_owned(),
                content_hash: content_hash.clone(),
                git_commit: format!("commit-{version}"),
                files,
            },
        );
        content_hash
    }

    fn publish_metas(registry: &Shared, region: &str, body: &str) {
        let etag = format!("\"{}\"", sha(body));
        registry
            .lock()
            .metas
            .insert(region.to_owned(), (body.to_owned(), etag));
    }

    async fn current(
        State(registry): State<Shared>,
        Path(region): Path<String>,
    ) -> axum::response::Response {
        let fake = registry.lock();
        fake.manifest_requests.fetch_add(1, Ordering::Relaxed);
        match fake.manifests.get(&region) {
            Some(manifest) => (
                [(header::CACHE_CONTROL, "no-cache")],
                sonic_rs::to_string(manifest).unwrap(),
            )
                .into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }

    async fn blob(
        State(registry): State<Shared>,
        Path((_region, sha256)): Path<(String, String)>,
    ) -> axum::response::Response {
        let fake = registry.lock();
        fake.blob_requests.fetch_add(1, Ordering::Relaxed);
        match fake.blobs.get(&sha256) {
            Some(body) => body.clone().into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }

    async fn metas(
        State(registry): State<Shared>,
        Path(region): Path<String>,
        headers: HeaderMap,
    ) -> axum::response::Response {
        let fake = registry.lock();
        fake.metas_requests.fetch_add(1, Ordering::Relaxed);
        let Some((body, etag)) = fake.metas.get(&region) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        if headers
            .get(header::IF_NONE_MATCH)
            .and_then(|value| value.to_str().ok())
            == Some(etag.as_str())
        {
            return StatusCode::NOT_MODIFIED.into_response();
        }
        ([(header::ETAG, etag.clone())], body.clone()).into_response()
    }

    async fn serve_fake_registry(registry: Shared) -> String {
        let app = Router::new()
            .route("/v1/master/{region}/current", get(current))
            .route("/v1/master/{region}/blob/{sha256}", get(blob))
            .route("/v1/metas/{region}/music_metas.json", get(metas))
            .with_state(registry);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    fn app_state(registry_url: Option<&str>) -> Arc<AppState> {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            DeckRecommend::init_data_path(concat!(env!("CARGO_MANIFEST_DIR"), "/_cpp_src/data"))
                .unwrap();
        });
        let registry = registry_url.map(|url| {
            let cfg = RegistryConfig::from_values(url, None, None, Some("2"), None).unwrap();
            Arc::new(RegistryClient::new(&cfg).unwrap())
        });
        Arc::new(AppState {
            engines: EnginePool::new(1).unwrap(),
            next_op_id: AtomicU64::new(0),
            debug: DebugConfig {
                lock_warn_threshold: Duration::from_secs(1),
                lock_timeout: Duration::from_secs(5),
                engine_warn_threshold: Duration::from_secs(5),
                default_recommend_timeout_ms: None,
                engine_thread_count: 1,
            },
            userdata_cache: UserdataCache::default(),
            registry,
            masterdata_state: Mutex::new(HashMap::new()),
        })
    }

    fn all_keys() -> Vec<&'static str> {
        REQUIRED_MASTERDATA_KEYS
            .iter()
            .chain(OPTIONAL_MASTERDATA_KEYS.iter())
            .copied()
            .collect()
    }

    fn counts(registry: &Shared) -> (usize, usize, usize) {
        let fake = registry.lock();
        (
            fake.manifest_requests.load(Ordering::Relaxed),
            fake.blob_requests.load(Ordering::Relaxed),
            fake.metas_requests.load(Ordering::Relaxed),
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ensure_region_loads_short_circuits_and_reloads_on_change() {
        let registry: Shared = Arc::default();
        let hash_v1 = publish(&registry, "jp", &all_keys(), "1.0.0");
        publish_metas(&registry, "jp", MUSIC_METAS_V1);
        let url = serve_fake_registry(registry.clone()).await;
        let state = app_state(Some(&url));

        // Full load: manifest + 37 blobs + metas.
        let outcome = ensure_region(&state, "JP", None, "preload").await.unwrap();
        assert!(outcome.reloaded);
        assert_eq!(outcome.state.content_hash, hash_v1);
        assert_eq!(outcome.state.git_commit, "commit-1.0.0");
        assert_eq!(outcome.state.data_version, "1.0.0");
        assert_eq!(outcome.state.source, "registry");
        assert_eq!(
            outcome.state.music_metas_digest.as_deref(),
            Some(digest_hex(MUSIC_METAS_V1).as_str())
        );
        assert!(outcome.state.loaded_at > 0);
        assert_eq!(counts(&registry), (1, 37, 1));
        assert!(state.masterdata_state.lock().contains_key("jp"));

        // Known hash: no round trip at all.
        let outcome = ensure_region(&state, "jp", Some(&hash_v1), "request")
            .await
            .unwrap();
        assert!(!outcome.reloaded);
        assert_eq!(counts(&registry), (1, 37, 1));

        // Unknown/absent hash: one manifest fetch, unchanged, metas 304.
        let outcome = ensure_region(&state, "jp", Some("stale"), "request")
            .await
            .unwrap();
        assert!(!outcome.reloaded);
        assert_eq!(counts(&registry), (2, 37, 2));

        // Music metas moved while master data did not.
        publish_metas(&registry, "jp", MUSIC_METAS_V2);
        let outcome = ensure_region(&state, "jp", None, "refresh").await.unwrap();
        assert!(!outcome.reloaded);
        assert_eq!(
            outcome.state.music_metas_digest.as_deref(),
            Some(digest_hex(MUSIC_METAS_V2).as_str())
        );
        assert_eq!(counts(&registry), (3, 37, 3));

        // New manifest without an optional key: reload, 36 blobs.
        let mut keys = all_keys();
        keys.retain(|key| *key != "ingameNotes");
        let hash_v2 = publish(&registry, "jp", &keys, "1.0.1");
        let outcome = ensure_region(&state, "jp", None, "refresh").await.unwrap();
        assert!(outcome.reloaded);
        assert_eq!(outcome.state.content_hash, hash_v2);
        assert_eq!(counts(&registry), (4, 37 + 36, 4));

        // A manifest missing a required key is rejected and nothing changes.
        let mut broken = all_keys();
        broken.retain(|key| *key != "cards");
        publish(&registry, "jp", &broken, "1.0.2");
        let err = ensure_region(&state, "jp", None, "refresh")
            .await
            .unwrap_err();
        assert!(matches!(err, RegistryError::MissingRequired(ref keys) if keys == &["cards"]));
        assert_eq!(
            state.masterdata_state.lock()["jp"].content_hash,
            hash_v2,
            "failed load must keep the previous state"
        );
        let app_err: AppError = err.into();
        assert!(matches!(app_err, AppError::Upstream(_)));

        // Regions the registry does not have surface as upstream errors.
        let err = ensure_region(&state, "en", None, "refresh")
            .await
            .unwrap_err();
        assert!(matches!(err, RegistryError::Status { status: 404, .. }));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handlers_report_state_and_configuration() {
        use crate::handlers::{masterdata_state, update_masterdata_from_registry};
        use crate::models::UpdateMasterdataFromRegistryRequest;

        let unconfigured = app_state(None);
        let err = update_masterdata_from_registry(
            State(unconfigured.clone()),
            axum::Json(UpdateMasterdataFromRegistryRequest {
                region: "jp".into(),
                content_hash: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, AppError::ServiceUnavailable(ref msg) if msg.contains("registry is not configured"))
        );
        let axum::Json(empty) = masterdata_state(State(unconfigured)).await;
        assert!(empty.registry_url.is_none());
        assert!(empty.regions.is_empty());

        let registry: Shared = Arc::default();
        let hash = publish(&registry, "cn", &all_keys(), "2.0.0");
        publish_metas(&registry, "cn", MUSIC_METAS_V1);
        let url = serve_fake_registry(registry.clone()).await;
        let state = app_state(Some(&url));
        let axum::Json(response) = update_masterdata_from_registry(
            State(state.clone()),
            axum::Json(UpdateMasterdataFromRegistryRequest {
                region: "cn".into(),
                content_hash: Some(hash.clone()),
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.status, "ok");
        assert_eq!(response.region, "cn");
        assert_eq!(response.content_hash, hash);
        assert!(response.reloaded);
        let text = sonic_rs::to_string(&response).unwrap();
        assert!(text.contains("\"contentHash\""), "{text}");
        assert!(text.contains("\"gitCommit\":\"commit-2.0.0\""), "{text}");
        assert!(text.contains("\"dataVersion\":\"2.0.0\""), "{text}");

        let axum::Json(snapshot) = masterdata_state(State(state.clone())).await;
        assert_eq!(snapshot.registry_url.as_deref(), Some(url.as_str()));
        assert_eq!(snapshot.regions["cn"].content_hash, hash);
        let text = sonic_rs::to_string(&snapshot).unwrap();
        assert!(text.contains("\"registryUrl\""), "{text}");
        assert!(text.contains("\"source\":\"registry\""), "{text}");
        assert!(text.contains("\"musicMetasDigest\""), "{text}");
        assert!(!text.contains("music_metas_etag"), "{text}");

        let err = update_masterdata_from_registry(
            State(state),
            axum::Json(UpdateMasterdataFromRegistryRequest {
                region: "  ".into(),
                content_hash: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));

        // Unreachable registry: a 502-class error, not a crash.
        let dead = app_state(Some("http://127.0.0.1:1"));
        let err = ensure_region(&dead, "jp", None, "preload")
            .await
            .unwrap_err();
        assert!(matches!(err, RegistryError::Http(_)));
        assert!(matches!(AppError::from(err), AppError::Upstream(_)));
    }
}
