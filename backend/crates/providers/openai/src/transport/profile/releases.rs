//! Platform releases share the request snapshot lock with the selected identity.
use super::selection::{ClientKind, ClientPlatform, ClientRelease};
use super::*;
use std::collections::VecDeque;

/// 每渠道保留的发布观察深度：当前版本加上全部滞后档位。
const RELEASE_HISTORY_CAPACITY: usize = selection::MAX_VERSION_LAG as usize + 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct ReleaseObservation {
    release: Option<ClientRelease>,
    /// 本渠道倒序观察到的发布序列（含当前版本），仅进程内存；重启后从种子与缓存重新积累。
    history: VecDeque<ClientRelease>,
    checked_at: Option<DateTime<Utc>>,
    error: Option<String>,
}

impl ReleaseObservation {
    pub(super) fn observe(&mut self, release: ClientRelease) {
        self.checked_at = Some(Utc::now());
        self.error = None;
        self.release = Some(release.clone());
        observe_release(&mut self.history, release);
    }
}

/// 观察到的发布进入历史：同版本刷新队首，新版本压入队首；超出深度丢弃最旧一项。
fn observe_release(history: &mut VecDeque<ClientRelease>, release: ClientRelease) {
    if history
        .front()
        .is_some_and(|front| front.same_identity(&release))
    {
        history[0] = release;
    } else {
        history.push_front(release);
    }
    while history.len() > RELEASE_HISTORY_CAPACITY {
        history.pop_back();
    }
}

fn release_key(client: ClientKind, platform: ClientPlatform, arch: &str) -> String {
    format!("{client:?}-{platform:?}-{arch}")
}

pub(super) fn release_cache_kind(provider: &ProviderKind, key: &str) -> ProviderKind {
    ProviderKind::new(format!("{}-{key}", provider.as_str())).expect("static release cache scope")
}

impl CodexWireProfileState {
    pub(super) fn seed_default_release(&self) {
        let profile = self.default_snapshot();
        self.seed_client_release(
            ClientKind::Desktop,
            ClientPlatform::Macos,
            "arm64",
            ClientRelease {
                codex_version: profile.codex_version,
                desktop_version: Some(profile.desktop_version),
                desktop_build: Some(profile.desktop_build),
                verified_at: Some(profile.verified_at),
            },
        );
    }

    pub fn client_release(
        &self,
        client: ClientKind,
        platform: ClientPlatform,
        arch: &str,
    ) -> Option<ClientRelease> {
        if client == ClientKind::Desktop && platform == ClientPlatform::Macos && arch == "arm64" {
            let profile = self.default_snapshot();
            return Some(ClientRelease {
                codex_version: profile.codex_version,
                desktop_version: Some(profile.desktop_version),
                desktop_build: Some(profile.desktop_build),
                verified_at: Some(profile.verified_at),
            });
        }
        self.selection
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .releases
            .get(&release_key(client, platform, arch))
            .and_then(|entry| entry.release.clone())
    }

    /// 按滞后档位解析发布：取观察历史的第 `lag` 项，历史不足时回退最旧一项。
    ///
    /// 滞后档位不引入新的失败模式：历史缺失时调用方回退到当前发布。
    pub fn lagged_client_release(
        &self,
        client: ClientKind,
        platform: ClientPlatform,
        arch: &str,
        lag: u32,
    ) -> Option<ClientRelease> {
        self.selection
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .releases
            .get(&release_key(client, platform, arch))
            .and_then(|state| {
                state
                    .history
                    .get(lag as usize)
                    .or_else(|| state.history.back())
                    .cloned()
            })
    }

    pub fn client_release_status(
        &self,
        client: ClientKind,
        platform: ClientPlatform,
        arch: &str,
    ) -> (Option<DateTime<Utc>>, Option<String>) {
        self.selection
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .releases
            .get(&release_key(client, platform, arch))
            .map_or((None, None), |state| {
                (state.checked_at, state.error.clone())
            })
    }

    pub(super) fn seed_client_release(
        &self,
        client: ClientKind,
        platform: ClientPlatform,
        arch: &str,
        release: ClientRelease,
    ) {
        let mut states = self
            .selection
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = states
            .releases
            .entry(release_key(client, platform, arch))
            .or_default();
        // 种子与缓存恢复同样构成观察：更晚恢复的版本会排在种子之前，形成启动历史。
        observe_release(&mut state.history, release.clone());
        state.release = Some(release);
    }

    pub(crate) fn record_client_release(
        &self,
        client: ClientKind,
        platform: ClientPlatform,
        arch: &str,
        result: Result<ClientRelease, String>,
    ) {
        let mut states = self
            .selection
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = states
            .releases
            .entry(release_key(client, platform, arch))
            .or_default();
        state.checked_at = Some(Utc::now());
        match result {
            Ok(release) => {
                state.release = Some(release.clone());
                state.error = None;
                observe_release(&mut state.history, release);
            }
            // 失败观察只更新状态，不改动发布历史。
            Err(error) => state.error = Some(error),
        }
    }
}
