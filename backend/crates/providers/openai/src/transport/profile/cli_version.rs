//! Stable CLI version observation, independent from Desktop artifact identity.

use super::*;
use std::time::{Instant, SystemTime};

const SYNC_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const MAX_RELEASE_BYTES: usize = 16 * 1024 * 1024;
const RELEASES: &str = "https://api.github.com/repos/openai/codex/releases";

pub(super) fn stable_version(value: &str) -> Option<semver::Version> {
    let version = semver::Version::parse(value).ok()?;
    (version.pre.is_empty() && version.build.is_empty()).then_some(version)
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}

fn latest(releases: Vec<Release>) -> Option<semver::Version> {
    releases
        .into_iter()
        .filter(|release| !release.draft && !release.prerelease)
        .filter_map(|release| stable_version(release.tag_name.strip_prefix("rust-v")?))
        .max()
}

fn sequence(version: &semver::Version) -> Option<u64> {
    if version.minor >= 1_000_000 || version.patch >= 1_000_000 {
        return None;
    }
    version
        .major
        .checked_mul(1_000_000_000_000)?
        .checked_add(version.minor.checked_mul(1_000_000)?)?
        .checked_add(version.patch)
        .filter(|value| *value > 0 && *value <= 9_007_199_254_740_991)
}

pub struct CodexCliVersionService {
    profile: CodexWireProfileState,
    cache: Arc<dyn ProviderArtifactProfileCachePort>,
    cache_kind: ProviderKind,
    client: Client,
    releases_url: String,
    last_attempt: tokio::sync::Mutex<Option<Instant>>,
}

impl CodexCliVersionService {
    pub fn new(
        profile: CodexWireProfileState,
        cache: Arc<dyn ProviderArtifactProfileCachePort>,
    ) -> Result<Self, reqwest::Error> {
        Ok(Self {
            profile,
            cache,
            releases_url: RELEASES.to_owned(),
            cache_kind: ProviderKind::new("openai-cli-version").expect("static cache scope"),
            client: Client::builder()
                .no_proxy()
                .redirect(Policy::none())
                .timeout(Duration::from_secs(30))
                .user_agent("NewCPR-version-sync")
                .build()?,
            last_attempt: tokio::sync::Mutex::new(None),
        })
    }

    // The host checks for a newly enabled policy every minute; network checks stay six-hourly.
    pub async fn refresh(&self) -> Result<(), &'static str> {
        if !self.profile.cli_sync_enabled() {
            return Ok(());
        }
        let mut last_attempt = self.last_attempt.lock().await;
        let cached = self
            .cache
            .read(&self.cache_kind)
            .await
            .map_err(|_| "CLI version cache read failed")?;
        let mut previous = None;
        if let Some(cached) = cached
            && let Some(version) = cached
                .profile()
                .expose_to_provider()
                .get("version")
                .and_then(Value::as_str)
                .and_then(stable_version)
            && sequence(&version) == Some(cached.artifact_sequence())
        {
            self.profile.update_cli_version(&version.to_string());
            previous = Some(version);
            if SystemTime::now()
                .duration_since(cached.verified_at())
                .is_ok_and(|age| age < SYNC_INTERVAL)
            {
                return Ok(());
            }
        }
        if last_attempt.is_some_and(|last| last.elapsed() < SYNC_INTERVAL) {
            return Ok(());
        }
        *last_attempt = Some(Instant::now());
        let candidate = tokio::time::timeout(Duration::from_secs(30), self.fetch())
            .await
            .map_err(|_| "CLI version lookup timed out")??;
        let candidate = previous.map_or(candidate.clone(), |previous| previous.max(candidate));
        let version = candidate.to_string();
        let profile = ProviderArtifactProfile::new(
            self.cache_kind.clone(),
            sequence(&candidate).ok_or("CLI version exceeds cache range")?,
            SystemTime::now(),
            OpaqueProviderData::new(Map::from_iter([(
                "version".to_owned(),
                Value::String(version.clone()),
            )])),
        );
        if self
            .cache
            .replace_if_newer(profile, ARTIFACT_PROFILE_CACHE_TTL)
            .await
            .map_err(|_| "CLI version cache write failed")?
        {
            self.profile.update_cli_version(&version);
        }
        Ok(())
    }

    async fn fetch(&self) -> Result<semver::Version, &'static str> {
        if let Ok(body) = self
            .fetch_json(&format!("{}/latest", self.releases_url))
            .await
            && let Ok(release) = serde_json::from_slice::<Release>(&body)
            && let Some(version) = latest(vec![release])
        {
            return Ok(version);
        }
        let body = self
            .fetch_json(&format!("{}?per_page=30", self.releases_url))
            .await?;
        latest(serde_json::from_slice(&body).map_err(|_| "Invalid CLI release list")?)
            .ok_or("No stable CLI release found")
    }

    async fn fetch_json(&self, url: &str) -> Result<Vec<u8>, &'static str> {
        let response = self
            .client
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|_| "CLI release request failed")?
            .error_for_status()
            .map_err(|_| "CLI release HTTP failure")?;
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| "CLI release read failed")?;
            if bytes.len().saturating_add(chunk.len()) > MAX_RELEASE_BYTES {
                return Err("CLI release response too large");
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MemoryCache(std::sync::Mutex<Option<ProviderArtifactProfile>>);

    impl ProviderArtifactProfileCachePort for MemoryCache {
        fn read<'a>(
            &'a self,
            _: &'a ProviderKind,
        ) -> BoxFuture<'a, Result<Option<ProviderArtifactProfile>, ProviderStoreError>> {
            Box::pin(async { Ok(self.0.lock().unwrap().clone()) })
        }
        fn replace_if_newer(
            &self,
            profile: ProviderArtifactProfile,
            _: Duration,
        ) -> BoxFuture<'_, Result<bool, ProviderStoreError>> {
            Box::pin(async move {
                let mut current = self.0.lock().unwrap();
                if current.as_ref().is_some_and(|current| {
                    current.artifact_sequence() > profile.artifact_sequence()
                }) {
                    return Ok(false);
                }
                *current = Some(profile);
                Ok(true)
            })
        }
    }

    fn auto_profile() -> CodexWireProfileState {
        let profile = CodexWireProfileState::new(CodexWireProfile::parse_user_agent(
            "Codex Desktop/0.153.0 (Mac OS 26.4.1; arm64) unknown (Codex Desktop; 26.1001.12345)"
        ).unwrap());
        profile
            .apply_user_agent_override(&ProviderUserAgentOverride::CustomAutoVersion {
                user_agent:
                    "codex_exec/0.153.0 (Mac OS 26.4.1; arm64) xterm-256color (codex_exec; 0.153.0)"
                        .to_owned(),
            })
            .unwrap();
        profile
    }

    #[tokio::test]
    async fn cli_sync_falls_back_caches_and_restores_without_repeated_network_requests() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
        let server = MockServer::start().await;
        Mock::given(path("/latest"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "tag_name":"other-v99.0.0", "draft":false, "prerelease":false
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"tag_name":"rust-v0.160.0", "draft":false, "prerelease":false}
            ])))
            .expect(1)
            .mount(&server)
            .await;
        let cache = Arc::new(MemoryCache::default());
        let profile = auto_profile();
        let mut service = CodexCliVersionService::new(profile.clone(), cache.clone()).unwrap();
        service.releases_url = server.uri();
        service.refresh().await.unwrap();
        service.refresh().await.unwrap();
        assert_eq!(profile.snapshot().codex_version, "0.160.0");
        let restored = auto_profile();
        let mut restarted = CodexCliVersionService::new(restored.clone(), cache).unwrap();
        restarted.releases_url = server.uri();
        restarted.refresh().await.unwrap();
        assert_eq!(restored.snapshot().codex_version, "0.160.0");
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn cli_sync_failure_keeps_version_and_respects_retry_interval() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::any};
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(503))
            .expect(2)
            .mount(&server)
            .await;
        let profile = auto_profile();
        profile.update_cli_version("0.160.0");
        let mut service =
            CodexCliVersionService::new(profile.clone(), Arc::new(MemoryCache::default())).unwrap();
        service.releases_url = server.uri();
        assert!(service.refresh().await.is_err());
        service.refresh().await.unwrap();
        assert_eq!(profile.snapshot().codex_version, "0.160.0");
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[test]
    fn stable_release_filter_ignores_unrelated_prereleases_and_orders_numerically() {
        let releases = serde_json::from_value(serde_json::json!([
            {"tag_name":"rust-v0.9.0","draft":false,"prerelease":false},
            {"tag_name":"rust-v0.10.0","draft":false,"prerelease":false},
            {"tag_name":"rust-v9.0.0-alpha","draft":false,"prerelease":false},
            {"tag_name":"rust-v8.0.0","draft":true,"prerelease":false},
            {"tag_name":"rust-v7.0.0","draft":false,"prerelease":true},
            {"tag_name":"rusty-v8-v99.0.0","draft":false,"prerelease":false}
        ]))
        .unwrap();
        assert_eq!(latest(releases).unwrap().to_string(), "0.10.0");
        assert!(sequence(&semver::Version::new(u64::MAX, 0, 0)).is_none());
        assert!(
            sequence(&semver::Version::new(1, 0, 0)) > sequence(&semver::Version::new(0, 999, 999))
        );
    }
}
