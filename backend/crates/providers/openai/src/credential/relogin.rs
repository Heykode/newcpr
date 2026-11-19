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
        "web_session": request.web_session,
        "source_ip": request.source_ip,
        "proxy": request.outbound_proxy.as_ref().map(|proxy| proxy.expose_url()),
    }))
    .map_err(|_| error("重登请求编码失败"))?;
    let payload = run_worker(input).await?;
    let session = worker_session(&payload, &request)?;
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
            return Err(error("有多个同级工作区，请选择后继续")
                .with_relogin_workspace_choices(choices)
                .with_relogin_web_session(session));
        }
        let stop_reason = match payload.get("code").and_then(Value::as_str) {
            Some("account_banned") => Some(ReloginStopReason::AccountBanned),
            Some("workspace_unavailable") => Some(ReloginStopReason::WorkspaceUnavailable),
            _ => None,
        };
        if let Some(reason) = stop_reason {
            return Err(error(reason.message())
                .with_relogin_stop_reason(reason)
                .with_relogin_web_session(session));
        }
        return Err(error(match payload.get("code").and_then(Value::as_str) {
            Some("runtime_missing") => "重登运行环境缺少 curl_cffi 或 pyotp",
            Some("workspace_missing") => "工作区目录未返回原工作区，请核对最新工作区列表",
            Some("session_response_invalid") => "会话响应格式异常，未使用错误响应触发重登",
            Some("password_rejected") => "账号密码被拒绝，请检查资料或账号状态",
            Some("mfa_rejected" | "mfa_factor_missing") => "2FA 验证失败，请检查密钥和服务器时间",
            Some("invalid_material") => "请重新导入邮箱、密码和有效的 2FA 密钥",
            Some("workspace_plan_unknown" | "workspace_unknown") => {
                "无法确认工作区套餐，请指定工作区 ID"
            }
            Some("identity_mismatch" | "plan_mismatch") => "新凭据身份、套餐或工作区不符，未推送",
            Some("email_verification_required") => {
                "上游仍要求邮箱验证码，请在官方页面完成验证后重试"
            }
            Some("interaction_required" | "login_loop") => {
                "登录需要人工交互或流程已变化，请人工重新授权"
            }
            Some("rate_limited") => "登录被限流，请稍后重试",
            Some("csrf_missing" | "session_missing" | "authorize_missing") => {
                "未能建立登录会话，请检查网络或登录页面变化"
            }
            Some("verification_failed" | "invalid_token") => "新凭据未通过验证，未推送",
            _ => "重登网络或上游验证失败，请检查代理或人工重新授权",
        })
        .with_relogin_web_session(session));
    }
    prepare_credential(provider, request, payload).await
}

fn worker_session(
    payload: &Value,
    request: &ReloginRequest,
) -> Result<Option<gateway_admin::model::relogin_maintenance::WebSession>, ProviderAdminError> {
    let mut session: Option<gateway_admin::model::relogin_maintenance::WebSession> = payload
        .get("web_session")
        .filter(|value| !value.is_null())
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| error("Web 会话格式不合法"))?;
    if session
        .as_ref()
        .is_some_and(|session| !session.email.eq_ignore_ascii_case(&request.email))
    {
        return Err(error("Web 会话账号不符"));
    }
    if let Some(session) = &mut session {
        session.source_ip = request.source_ip;
        session.egress_mode = request.egress_mode;
    }
    Ok(session)
}

async fn prepare_credential(
    provider: &OpenAiAdminProvider,
    request: ReloginRequest,
    payload: Value,
) -> Result<ReloginCredential, ProviderAdminError> {
    let web_session = worker_session(&payload, &request)?;
    let document = payload
        .get("document")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| error("重登缺少凭据"))?;
    let prepared = provider
        .prepare_import(PrepareCredentialImport {
            credential_outbound_proxy: None,
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
        web_session,
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

async fn run_worker(input: Vec<u8>) -> Result<Value, ProviderAdminError> {
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
    tokio::time::timeout(Duration::from_secs(270), io)
        .await
        .map_err(|_| error("重登超时，请检查网络或代理"))?
}

pub(crate) async fn maintain(
    provider: &OpenAiAdminProvider,
    request: gateway_admin::model::relogin_maintenance::MaintenanceRequest,
) -> Result<gateway_admin::model::relogin_maintenance::MaintenanceOutcome, ProviderAdminError> {
    use gateway_admin::model::relogin_maintenance::*;
    let input = serde_json::to_vec(&serde_json::json!({
        "email": request.login.email, "password": request.login.password,
        "mfa_secret": request.login.mfa_secret, "workspace_id": request.login.workspace_id,
        "source_ip": request.login.source_ip,
        "proxy": request.login.outbound_proxy.as_ref().map(|proxy| proxy.expose_url()),
        "web_session": request.login.web_session, "action": request.action,
        "target": request.target, "device_id_hash": request.device_id_hash,
        "pending_totp": request.pending_totp,
    }))
    .map_err(|_| error("维护请求编码失败"))?;
    let payload = run_worker(input).await?;
    let succeeded = payload.get("ok").and_then(Value::as_bool) == Some(true);
    let ambiguous = payload.get("code").and_then(Value::as_str) == Some("workspace_ambiguous");
    let mut view: MaintenanceView = serde_json::from_value(
        payload
            .get("view")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
    )
    .map_err(|_| error("维护结果格式不合法"))?;
    view.action = Some(request.action);
    view.checked_at = Some(chrono::Utc::now());
    view.state = if succeeded {
        "succeeded"
    } else if payload.get("uncertain").and_then(Value::as_bool) == Some(true) {
        "uncertain"
    } else if ambiguous && request.login.workspace_id.is_none() {
        "awaiting_workspace"
    } else {
        "failed"
    }
    .to_owned();
    if !succeeded {
        let code = payload.get("code").and_then(Value::as_str).unwrap_or("");
        view.message = match code {
            "workspace_ambiguous" => "发现多个同级工作区，请选择工作区后继续授权",
            "session_response_invalid" => "会话响应格式异常，请稍后重新检测",
            "session_missing" => "尚无 Web 会话，请先重登",
            "session_expired" => "Web 会话已失效，请重新授权或强制重登",
            "reauthentication_required" => {
                "上游要求重新验证身份，操作未执行；请先强制重登，再重试此安全操作"
            }
            "identity_mismatch" => "上游返回的账号身份不符，未替换资料",
            "workspace_missing" => "工作区目录未返回目标空间，请查看最新工作区列表",
            "workspace_unavailable" => "目标工作区明确不可访问，未切换其他空间",
            "device_missing" => "设备不存在或是当前设备，请刷新列表",
            "totp_already_enabled" => "上游仍启用 2FA，请先核对并停用旧密钥",
            "totp_enroll_missing" => "没有待激活的 2FA，请先准备密钥",
            "totp_activation_unconfirmed" | "totp_disable_unconfirmed" => {
                "2FA 操作结果未确认，请核对上游状态，勿重复提交"
            }
            "password_interaction_required" => {
                "密码维护需要上游再次验证，请在官方安全设置中完成后更新本地资料"
            }
            "mfa_factor_missing" => "上游未返回唯一的 TOTP 因子，未执行停用",
            "interaction_required" => "上游要求人工验证，请在官方页面完成",
            "email_verification_required" => "上游仍要求邮箱验证码，请在官方页面完成验证后重试",
            "password_rejected" => "密码验证失败，请检查本地资料",
            "mfa_rejected" => "2FA 验证失败，请检查密钥与服务器时间",
            "rate_limited" => "上游限流，请稍后重试",
            _ => "维护未完成，请检查网络或在官方页面核对",
        }
        .to_owned();
        if code == "session_expired" {
            view.session_state = "expired".to_owned();
        }
    } else if view.message.is_empty() {
        view.message = format!("{}已完成", request.action.label());
    }
    if ambiguous && request.login.workspace_id.is_none() {
        let choices: Vec<ReloginWorkspaceChoice> =
            serde_json::from_value(payload.get("workspaces").cloned().unwrap_or(Value::Null))
                .map_err(|_| error("工作区候选格式不合法"))?;
        validate_workspace_choices(&choices).map_err(|_| error("工作区候选格式不合法"))?;
        view.workspaces = choices
            .into_iter()
            .map(|choice| MaintenanceWorkspace {
                id: choice.id,
                name: choice.name,
                plan_type: Some(choice.plan_type),
                available: true,
            })
            .collect();
    }
    let session = worker_session(&payload, &request.login)?;
    if let Some(session) = &session {
        view.session_state = "valid".to_owned();
        view.session_checked_at = Some(session.verified_at);
    }
    let pending_totp = payload
        .get("pending_totp")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| error("2FA 候选格式不合法"))?;
    let new_secret = payload
        .get("new_secret")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let credential = if succeeded && payload.get("document").is_some() {
        Some(prepare_credential(provider, request.login, payload).await?)
    } else {
        None
    };
    Ok(MaintenanceOutcome {
        view,
        session,
        credential,
        pending_totp,
        new_secret,
    })
}
