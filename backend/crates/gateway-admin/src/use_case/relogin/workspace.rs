use super::*;
use crate::model::provider_credentials::{CredentialRotationCommit, PreparedCredentialRotation};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushTargetView {
    pub account_id: String,
    pub workspace_id: String,
    pub plan_type: Option<String>,
    pub switch_workspace: bool,
    pub available: bool,
}

pub(super) fn prepare_queue(
    entry: &mut ReloginEntry,
    pool: &[AccountRecord],
    mode: ReloginWorkspaceMode,
) -> Result<(), AdminError> {
    let targets = if mode == ReloginWorkspaceMode::Highest {
        let matches = matching_accounts(entry, pool);
        if let Some(first) = matches.first()
            && matches
                .iter()
                .any(|account| account.outbound_proxy != first.outbound_proxy)
        {
            return Err(AdminError::conflict(
                "同邮箱账号使用不同代理，请选择原工作区重登",
            ));
        }
        matches
            .into_iter()
            .map(ReloginTarget::from_account)
            .collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };
    entry.target = if mode == ReloginWorkspaceMode::Original {
        select_target(entry, pool)?
    } else {
        None
    };
    entry.workspace_mode = mode;
    entry.workspace_targets = targets;
    Ok(())
}

pub(super) fn push_targets(entry: &ReloginEntry, pool: &[AccountRecord]) -> Vec<PushTargetView> {
    if entry.workspace_mode != ReloginWorkspaceMode::Highest {
        return Vec::new();
    }
    let Some(credential) = &entry.credential else {
        return Vec::new();
    };
    // Prefer the already-existing destination; never replace another row with its identity.
    let existing = pool.iter().find(|account| {
        account.upstream_user_id.as_deref() == Some(credential.user_id.as_str())
            && account.upstream_account_id.as_deref() == Some(credential.workspace_id.as_str())
    });
    entry
        .workspace_targets
        .iter()
        .filter(|target| target.user_id == credential.user_id)
        .map(|target| {
            let current = pool.iter().find(|account| account.id == target.account_id);
            PushTargetView {
                account_id: target.account_id.clone(),
                workspace_id: target.workspace_id.clone(),
                plan_type: current.and_then(|account| account.plan_type.clone()),
                switch_workspace: target.workspace_id != credential.workspace_id,
                available: current.is_some_and(|account| {
                    target.matches_account(account)
                        && account
                            .email
                            .as_ref()
                            .is_some_and(|email| email.eq_ignore_ascii_case(&entry.email))
                }) && existing.is_none_or(|account| account.id == target.account_id),
            }
        })
        .collect()
}

pub(super) fn confirm_target(
    entry: &mut ReloginEntry,
    selection: Option<&ReloginPushSelection>,
) -> Result<(), AdminError> {
    if entry.workspace_mode != ReloginWorkspaceMode::Highest {
        if selection.is_some() {
            return Err(AdminError::conflict("原工作区重登不能切换推送目标"));
        }
        return Ok(());
    }
    if entry.automatic_job || entry.manual_push_context.is_some() {
        return Err(AdminError::conflict("自动恢复不能切换工作区"));
    }
    if entry.workspace_targets.is_empty() {
        if selection.is_some() {
            return Err(AdminError::conflict("本次获取未锁定已有账号，请重新获取"));
        }
        return Ok(());
    }
    let selection = selection.ok_or_else(|| AdminError::conflict("请确认推送目标和工作区切换"))?;
    let target = entry
        .workspace_targets
        .iter()
        .find(|target| target.account_id == selection.account_id)
        .ok_or_else(|| AdminError::conflict("所选账号不在本次获取的目标中"))?;
    let credential = entry
        .credential
        .as_ref()
        .ok_or_else(|| AdminError::conflict("请先获取新凭据"))?;
    if target.user_id != credential.user_id || credential.user_id.is_empty() {
        return Err(AdminError::conflict("新凭据不是原账号的同一用户"));
    }
    if selection.switch_workspace != (target.workspace_id != credential.workspace_id) {
        return Err(AdminError::conflict(
            "工作区切换确认不匹配，请刷新后重新确认",
        ));
    }
    entry.target = Some(target.clone());
    Ok(())
}

impl DefaultReloginService {
    pub(super) async fn commit_workspace_switch(
        &self,
        prepared: PreparedCredentialRotation,
        context: &MutationContext,
        operation_id: String,
    ) -> Result<CredentialMutationResult, AdminError> {
        let (facts, guard) = prepared.into_parts();
        match self
            .accounts
            .commit_relogin_workspace_switch(
                CredentialRotationCommit {
                    prepared: facts,
                    relogin_operation_id: Some(operation_id),
                },
                context,
            )
            .await
        {
            Ok(result) => {
                guard.finish();
                Ok(result)
            }
            Err(error) => {
                drop(guard);
                Err(map_store_error(error, "relogin workspace"))
            }
        }
    }
}
