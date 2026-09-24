//! Isolated login runtime. Credentials are delivered through stdin, never argv.

use crate::admin::OpenAiAdminProvider;
use gateway_admin::model::provider_credentials::ProviderDocument;
use gateway_admin::{
    model::{
        provider_credentials::PrepareCredentialImport,
        relogin::{
            ReloginCredential, ReloginRequest, ReloginStopReason, ReloginWorkspaceChoice,
            validate_workspace_choices,
        },
    },
    ports::provider::{ProviderAdmin, ProviderAdminError, ProviderAdminErrorKind},
};
use gateway_core::account::OpaqueProviderData;
use serde_json::Value;
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

const WORKER: &str = include_str!("relogin_worker.py");
const MAX_OUTPUT: u64 = 1024 * 1024;

fn error(message: &'static str) -> ProviderAdminError {
    ProviderAdminError::new(ProviderAdminErrorKind::Unavailable).with_public_message(message)
}

pub(crate) async fn login(
    provider: &OpenAiAdminProvider,
    request: ReloginRequest,
) -> Result<ReloginCredential, ProviderAdminError> {
    let input = serde_json::to_vec(&serde_json::json!({
        "email": request.email,
        "password": request.password,
        "mfa_secret": request.mfa_secret,
        "workspace_id": request.workspace_id,
        "proxy": request.outbound_proxy.as_ref().map(|proxy| proxy.expose_url()),
    }))
    .map_err(|_| error("重登请求编码失败"))?;
    let mut child = Command::new("python3")
        .args(["-I", "-c", WORKER])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| error("未找到 Python 重登运行环境"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| error("重登进程输入不可用"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| error("重登进程输出不可用"))?;
    let io = async {
        stdin
            .write_all(&input)
            .await
            .map_err(|_| error("重登进程输入失败"))?;
        drop(stdin);
        let mut output = Vec::new();
        stdout
            .take(MAX_OUTPUT + 1)
            .read_to_end(&mut output)
            .await
            .map_err(|_| error("重登进程读取失败"))?;
        if output.len() as u64 > MAX_OUTPUT {
            return Err(error("重登结果超出大小限制"));
        }
        if !child
            .wait()
            .await
            .map_err(|_| error("重登进程异常"))?
            .success()
        {
            return Err(error("重登进程异常退出"));
        }
        serde_json::from_slice::<Value>(&output).map_err(|_| error("重登返回格式不合法"))
    };
    let payload = tokio::time::timeout(Duration::from_secs(270), io)
        .await
        .map_err(|_| error("重登超时，请检查网络或代理"))??;
    if payload.get("ok").and_then(Value::as_bool) != Some(true) {
        if payload.get("code").and_then(Value::as_str) == Some("workspace_ambiguous") {
            // An explicitly locked workspace must never become a switching prompt.
            if request.workspace_id.is_some() {
                return Err(error("指定工作区返回了不明确的结果，请重新获取"));
            }
            let choices: Vec<ReloginWorkspaceChoice> =
                serde_json::from_value(payload.get("workspaces").cloned().unwrap_or(Value::Null))
                    .map_err(|_| error("重登工作区候选格式不合法"))?;
            validate_workspace_choices(&choices).map_err(|_| error("重登工作区候选格式不合法"))?;
            return Err(
                error("有多个同级工作区，请选择后继续").with_relogin_workspace_choices(choices)
            );
        }
        let stop_reason = match payload.get("code").and_then(Value::as_str) {
            Some("account_banned") => Some(ReloginStopReason::AccountBanned),
            Some("workspace_missing" | "workspace_unavailable") => {
                Some(ReloginStopReason::WorkspaceUnavailable)
            }
            _ => None,
        };
        if let Some(reason) = stop_reason {
            return Err(error(reason.message()).with_relogin_stop_reason(reason));
        }
        return Err(error(match payload.get("code").and_then(Value::as_str) {
            Some("runtime_missing") => "重登运行环境缺少 curl_cffi 或 pyotp",
            Some("password_rejected") => "账号密码被拒绝，请检查资料或账号状态",
            Some("mfa_rejected" | "mfa_factor_missing") => "2FA 验证失败，请检查密钥和服务器时间",
            Some("invalid_material") => "请重新导入邮箱、密码和有效的 2FA 密钥",
            Some("workspace_plan_unknown" | "workspace_unknown") => {
                "无法确认工作区套餐，请指定工作区 ID"
            }
            Some("identity_mismatch" | "plan_mismatch") => "新凭据身份、套餐或工作区不符，未推送",
            Some("interaction_required" | "login_loop") => {
                "登录需要人工交互或流程已变化，请人工重新授权"
            }
            Some("rate_limited") => "登录被限流，请稍后重试",
            Some("csrf_missing" | "session_missing" | "authorize_missing") => {
                "未能建立登录会话，请检查网络或登录页面变化"
            }
            Some("verification_failed" | "invalid_token") => "新凭据未通过验证，未推送",
            _ => "重登网络或上游验证失败，请检查代理或人工重新授权",
        }));
    }
    let document = payload
        .get("document")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| error("重登缺少凭据"))?;
    let prepared = provider
        .prepare_import(PrepareCredentialImport {
            default_outbound_proxy: request.outbound_proxy,
            document: ProviderDocument::new(OpaqueProviderData::new(document.clone())),
        })
        .await?;
    if prepared.credentials.len() != 1 {
        return Err(error("重登返回了不明确的账号"));
    }
    let account = prepared
        .credentials
        .into_iter()
        .next()
        .ok_or_else(|| error("重登未返回账号"))?;
    let email = account.email.ok_or_else(|| error("凭据缺少邮箱"))?;
    let workspace_id = account
        .upstream_account_id
        .ok_or_else(|| error("凭据缺少工作区"))?;
    if !email.eq_ignore_ascii_case(&request.email)
        || request
            .workspace_id
            .as_ref()
            .is_some_and(|target| target != &workspace_id)
    {
        return Err(error("重登身份与原账号不一致"));
    }
    Ok(ReloginCredential {
        document,
        email,
        workspace_id,
        user_id: account
            .upstream_user_id
            .ok_or_else(|| error("凭据缺少用户身份"))?,
        plan_type: account.plan_type.ok_or_else(|| error("凭据缺少套餐"))?,
        expires_at: account
            .access_token_expires_at
            .ok_or_else(|| error("凭据缺少有效期"))?,
        verified_at: chrono::Utc::now(),
    })
}
