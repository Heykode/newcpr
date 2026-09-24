use super::*;

const AUTOMATIC_WINDOW_MINUTES: i64 = 15;
const MAX_AUTOMATIC_STARTS: usize = 3;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryView {
    pub state: &'static str,
    pub message: String,
    pub retry_at: Option<DateTime<Utc>>,
    pub retries_used: u32,
    pub max_retries: u32,
}

impl RecoveryView {
    fn new(state: &'static str, message: impl Into<String>) -> Self {
        Self {
            state,
            message: message.into(),
            retry_at: None,
            retries_used: 0,
            max_retries: 0,
        }
    }

    pub fn waiting(&self) -> bool {
        self.state == "waiting"
    }
}

pub(super) fn recent_starts(entry: &ReloginEntry, now: DateTime<Utc>) -> Vec<DateTime<Utc>> {
    entry
        .automatic_started_at
        .iter()
        .copied()
        .filter(|at| *at > now - chrono::Duration::minutes(AUTOMATIC_WINDOW_MINUTES))
        .collect()
}

pub(super) fn same_attempt(entry: &ReloginEntry, account: &AccountRecord) -> bool {
    entry
        .attempted_target
        .as_ref()
        .or(entry.target.as_ref())
        .is_some_and(|target| target.matches_account(account))
}

pub(super) fn view(
    entry: &ReloginEntry,
    pool: &[AccountRecord],
    settings: &ReloginSettings,
    now: DateTime<Utc>,
) -> RecoveryView {
    let mut result = eligibility(entry, pool, settings, now);
    result.retries_used = if entry.synced_at.is_none() {
        entry.automatic_attempts.saturating_sub(1)
    } else {
        0
    };
    result.max_retries = settings.max_retries;
    result
}

fn eligibility(
    entry: &ReloginEntry,
    pool: &[AccountRecord],
    settings: &ReloginSettings,
    now: DateTime<Utc>,
) -> RecoveryView {
    if matches!(
        entry.status,
        ReloginStatus::Uncertain | ReloginStatus::Pushing
    ) {
        return RecoveryView::new("uncertain", "推送结果待核实，不自动重复推送");
    }
    if entry.validate_totp().is_err() {
        return RecoveryView::new("invalid_material", "缺少有效的密码或 2FA 资料");
    }
    if entry.status == ReloginStatus::AwaitingWorkspace {
        return RecoveryView::new("awaiting_workspace", "请选择工作区后继续获取凭据");
    }
    if settings.paused {
        return RecoveryView::new("paused", "重登队列已暂停");
    }
    if entry.status == ReloginStatus::Running {
        return RecoveryView::new("running", "正在重登");
    }
    if entry.status == ReloginStatus::Queued {
        return RecoveryView::new("waiting", "已排队，等待可用并发");
    }
    if !entry.automatic {
        return RecoveryView::new("disabled", "自动重登已关闭");
    }
    if let Some(reason) = entry.automatic_stop_reason() {
        return RecoveryView::new("manual_required", reason.label());
    }
    let target = match select_target(entry, pool) {
        Ok(Some(target)) => target,
        Ok(None) if !matching_accounts(entry, pool).is_empty() => {
            return RecoveryView::new("workspace_required", "指定工作区不在号池中");
        }
        Ok(None) => return RecoveryView::new("not_in_pool", "尚未入池，等待手动获取凭据"),
        Err(error) => return RecoveryView::new("workspace_required", error.message()),
    };
    let Some(account) = pool.iter().find(|account| account.id == target.account_id) else {
        return RecoveryView::new("not_in_pool", "原账号已删除");
    };
    if !account.enabled {
        return RecoveryView::new("account_disabled", "号池账号已暂停调度");
    }
    if account.credential_state == CredentialState::Banned
        || account.last_error_reason
            == Some(gateway_core::account::AccountErrorReason::AccountBanned)
    {
        return RecoveryView::new("manual_required", ReloginStopReason::AccountBanned.label());
    }
    if !worker::needs_relogin(account) {
        return if account.credential_state == CredentialState::Ready {
            RecoveryView::new("idle", "监听账号失效")
        } else {
            RecoveryView::new("unsupported_error", "当前异常不属于可自动重登的凭据过期")
        };
    }
    if entry.status == ReloginStatus::Ready
        && entry
            .credential
            .as_ref()
            .is_some_and(|credential| credential.expires_at > now)
        && entry.synced_at.is_none()
        && !entry.automatic_job
    {
        return RecoveryView::new("awaiting_push", "新凭据已获取，等待手动确认推送");
    }
    // A successful delivery, or a genuinely new auth binding, ends the old retry budget.
    // Routine Cookie revisions still match the captured authentication generation.
    if entry.synced_at.is_none() && same_attempt(entry, account) {
        if entry.automatic_attempts > settings.max_retries {
            return RecoveryView::new(
                "retry_limit",
                if settings.max_retries == 0 {
                    "首次自动重登失败，已设置不重试，请人工处理".to_owned()
                } else {
                    format!(
                        "已用完 {} 次自动重试，请检查失败原因后手动重登",
                        settings.max_retries
                    )
                },
            );
        }
        if let Some(at) = entry.next_attempt_at.filter(|at| *at > now) {
            return RecoveryView {
                state: "cooldown",
                message: "上次恢复未完成，等待重试".to_owned(),
                retry_at: Some(at),
                ..RecoveryView::new("cooldown", "")
            };
        }
    }
    let mut starts = recent_starts(entry, now);
    if starts.len() >= MAX_AUTOMATIC_STARTS {
        starts.sort();
        return RecoveryView {
            state: "loop_guard",
            message: "短时间反复失效，自动重登暂缓".to_owned(),
            retry_at: Some(
                starts[starts.len() - MAX_AUTOMATIC_STARTS]
                    + chrono::Duration::minutes(AUTOMATIC_WINDOW_MINUTES),
            ),
            ..RecoveryView::new("loop_guard", "")
        };
    }
    RecoveryView::new("waiting", "账号凭据已失效，等待可用并发")
}
