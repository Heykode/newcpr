//! 管理员认证状态机。

use std::sync::Arc;

use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;
use uuid::Uuid;

use crate::{
    model::{
        AdminError, AdminErrorKind,
        auth::{
            AdminAuditEvent, AdminSession, AuditActorKind, ChangePassword, LoginCommand,
            LoginError, LoginResult,
        },
    },
    ports::store::AuthStore,
};

use super::map_store_error;
mod session;

/// API 鉴权与管理员登录消费的统一服务。
#[async_trait]
pub trait AuthService: Send + Sync {
    async fn renew_session(
        &self,
        session_id: Option<&str>,
    ) -> Result<Option<LoginResult>, AdminError>;
    async fn change_password(
        &self,
        session_id: Option<&str>,
        command: ChangePassword,
    ) -> Result<(), AdminError>;
    async fn ensure_default_admin(&self, password: &str) -> Result<bool, AdminError>;
    async fn resolve_admin_user_id(
        &self,
        session_id: Option<&str>,
    ) -> Result<Option<String>, AdminError>;
    async fn verify_admin_api_key(&self, key: &str) -> Result<bool, AdminError>;
    async fn login(&self, command: LoginCommand) -> Result<LoginResult, LoginError>;
    async fn validate_session(&self, session_id: Option<&str>) -> Result<bool, AdminError>;
    async fn logout(&self, session_id: &str) -> Result<(), AdminError>;
}

/// 会话有效期上限(366 天);超出的配置按上限截断,保证 TTL 构造与
/// `Utc::now() + session_ttl` 永不越界。
const MAX_SESSION_TTL_MINUTES: i64 = 366 * 24 * 60;

/// 单管理员认证策略的最终实现。
pub(crate) struct DefaultAuthService {
    default_admin_user_id: String,
    session_ttl: Duration,
    session_absolute_ttl: Duration,
    store: Arc<dyn AuthStore>,
}

impl DefaultAuthService {
    async fn current_session(&self, session_id: &str) -> Result<Option<AdminSession>, AdminError> {
        let credential = session::Credential::parse(session_id);
        let Some(session) = self
            .load_live_session(session::storage_id(session_id))
            .await?
        else {
            return Ok(None);
        };
        let valid = match (&session.renewal, credential) {
            (Some(renewal), Some(credential)) => !credential.refresh && credential.owns(renewal),
            (None, None) => true,
            _ => false,
        };
        Ok((valid && session_valid(&session)).then_some(session))
    }

    fn access_ttl(&self) -> Duration {
        (self.session_ttl / 4).min(Duration::hours(24))
    }

    async fn load_live_session(
        &self,
        session_id: &str,
    ) -> Result<Option<AdminSession>, AdminError> {
        let Some(session) = self
            .store
            .load_session(session_id)
            .await
            .map_err(|error| map_store_error(error, "administrator session"))?
            .filter(|session| {
                session.retained_until() > Utc::now()
                    && session
                        .absolute_expires_at
                        .is_none_or(|limit| limit > Utc::now())
            })
        else {
            return Ok(None);
        };
        let hash = self
            .store
            .load_password_hash(&session.admin_user_id)
            .await
            .map_err(|error| map_store_error(error, "administrator"))?;
        // Keep the current PostgreSQL password hash authoritative for revocation.
        Ok(hash
            .filter(|hash| password_fingerprint(hash) == session.credential_fingerprint)
            .map(|_| session))
    }

    #[must_use]
    pub(crate) fn new(
        default_admin_user_id: impl Into<String>,
        session_ttl_minutes: u64,
        session_absolute_ttl_minutes: u64,
        store: Arc<dyn AuthStore>,
    ) -> Self {
        let minutes = i64::try_from(session_ttl_minutes)
            .unwrap_or(MAX_SESSION_TTL_MINUTES)
            .clamp(1, MAX_SESSION_TTL_MINUTES);
        Self {
            default_admin_user_id: default_admin_user_id.into(),
            session_ttl: Duration::minutes(minutes),
            session_absolute_ttl: Duration::minutes(
                i64::try_from(session_absolute_ttl_minutes)
                    .unwrap_or(MAX_SESSION_TTL_MINUTES)
                    .clamp(1, MAX_SESSION_TTL_MINUTES),
            ),
            store,
        }
    }

    fn auth_audit(&self, action: &str, occurred_at: chrono::DateTime<Utc>) -> AdminAuditEvent {
        AdminAuditEvent {
            id: format!("audit_{}", Uuid::now_v7().simple()),
            actor_kind: AuditActorKind::AdminSession,
            actor_admin_user_id: Some(self.default_admin_user_id.clone()),
            actor_ref: crate::model::auth::admin_session_actor_ref(&self.default_admin_user_id),
            request_id: None,
            action: action.to_owned(),
            entity_kind: "admin_session".to_owned(),
            entity_ref: self.default_admin_user_id.clone(),
            config_revision: None,
            changed_fields: Vec::new(),
            occurred_at,
        }
    }
}

#[async_trait]
impl AuthService for DefaultAuthService {
    async fn renew_session(
        &self,
        session_id: Option<&str>,
    ) -> Result<Option<LoginResult>, AdminError> {
        let Some(session_id) = session_id else {
            return Ok(None);
        };
        if session::Credential::parse(session_id).is_some() {
            return self.refresh(session_id).await;
        }
        Ok(self
            .current_session(session_id)
            .await?
            .map(|session| LoginResult {
                session_id: session_id.to_owned(),
                expires_at: session.expires_at,
                refresh_token: None,
                refresh_expires_at: None,
            }))
    }

    async fn change_password(
        &self,
        session_id: Option<&str>,
        command: ChangePassword,
    ) -> Result<(), AdminError> {
        let session = match session_id {
            Some(id) => self.current_session(id).await?,
            None => None,
        }
        .filter(session_valid)
        .ok_or_else(|| AdminError::new(AdminErrorKind::Unauthorized, "请先登录"))?;
        let hash = self
            .store
            .load_password_hash(&session.admin_user_id)
            .await
            .map_err(|error| map_store_error(error, "administrator"))?
            .filter(|hash| password_fingerprint(hash) == session.credential_fingerprint)
            .ok_or_else(|| {
                AdminError::new(AdminErrorKind::Unauthorized, "登录已失效，请重新登录")
            })?;
        // 按管理员限速，多个浏览器会话不能分别获得额外的尝试额度。
        if !self
            .store
            .consume_password_change_attempt(&session.admin_user_id, 10, 900)
            .await
            .map_err(|error| map_store_error(error, "password change limit"))?
        {
            return Err(AdminError::new(
                AdminErrorKind::RateLimited,
                "尝试过于频繁，请稍后再试",
            ));
        }
        validate_new_password(&command.new_password)?;
        if command.current_password.len() > 4096
            || !verify_admin_password(&command.current_password, &hash)?
        {
            return Err(AdminError::invalid("当前密码不正确"));
        }
        if command.current_password == command.new_password {
            return Err(AdminError::invalid("新密码不能与当前密码相同"));
        }
        let replacement = hash_admin_password(&command.new_password)?;
        let mut audit = self.auth_audit("admin.password_changed", Utc::now());
        audit.actor_admin_user_id = Some(session.admin_user_id.clone());
        audit.actor_ref = crate::model::auth::admin_session_actor_ref(&session.admin_user_id);
        audit.entity_kind = "admin_user".to_owned();
        audit.entity_ref = session.admin_user_id.clone();
        audit.changed_fields = vec!["password".to_owned()];
        if !self
            .store
            .change_password(&session.admin_user_id, &hash, &replacement, audit)
            .await
            .map_err(|error| map_store_error(error, "administrator password"))?
        {
            return Err(AdminError::conflict("密码已变更，请重新登录"));
        }
        Ok(())
    }

    async fn ensure_default_admin(&self, password: &str) -> Result<bool, AdminError> {
        let hash = hash_admin_password(password)?;
        self.store
            .create_password_hash_if_absent(&self.default_admin_user_id, &hash)
            .await
            .map_err(|error| map_store_error(error, "administrator"))
    }

    async fn resolve_admin_user_id(
        &self,
        session_id: Option<&str>,
    ) -> Result<Option<String>, AdminError> {
        let Some(session_id) = session_id else {
            return Ok(None);
        };
        Ok(self
            .current_session(session_id)
            .await?
            .map(|session| session.admin_user_id))
    }

    async fn verify_admin_api_key(&self, key: &str) -> Result<bool, AdminError> {
        if !valid_admin_api_key_shape(key) {
            return Ok(false);
        }
        let stored = self
            .store
            .load_admin_api_key()
            .await
            .map_err(|error| map_store_error(error, "administrator API key"))?;
        Ok(stored.as_ref().is_some_and(|stored| {
            let stored = stored.expose_for_auth();
            key.len() == stored.len() && bool::from(key.as_bytes().ct_eq(stored.as_bytes()))
        }))
    }

    async fn login(&self, command: LoginCommand) -> Result<LoginResult, LoginError> {
        if command
            .username
            .as_deref()
            .unwrap_or(&self.default_admin_user_id)
            != self.default_admin_user_id
        {
            return Err(LoginError::InvalidCredentials);
        }
        let hash = self
            .store
            .load_password_hash(&self.default_admin_user_id)
            .await
            .map_err(|error| LoginError::Unavailable(Some(error.into())))?
            .ok_or(LoginError::InvalidCredentials)?;
        if !verify_admin_password(&command.password, &hash)
            .map_err(|error| LoginError::Unavailable(Some(error.into())))?
        {
            return Err(LoginError::InvalidCredentials);
        }

        let now = Utc::now();
        let absolute_expires_at = now + self.session_absolute_ttl;
        let refresh_expiry = (now + self.session_ttl).min(absolute_expires_at);
        let expires_at = (now + self.access_ttl()).min(refresh_expiry);
        let (session_id, session, result) = session::issue(
            AdminSession {
                admin_user_id: self.default_admin_user_id.clone(),
                expires_at,
                credential_fingerprint: password_fingerprint(&hash),
                absolute_expires_at: Some(absolute_expires_at),
                renewal: None,
            },
            refresh_expiry,
        )
        .map_err(|error| LoginError::Unavailable(Some(error.into())))?;
        self.store
            .store_session(&session_id, &session)
            .await
            .map_err(|error| LoginError::Unavailable(Some(error.into())))?;
        if let Err(error) = self
            .store
            .append_audit_event(self.auth_audit("admin.login", Utc::now()))
            .await
        {
            let source = gateway_core::error::ErrorSource::new(error);
            let source = match self.store.delete_session(&session_id).await {
                Ok(_) => source,
                Err(cleanup) => source.with_cleanup(cleanup),
            };
            return Err(LoginError::Unavailable(Some(source)));
        }
        Ok(result)
    }

    async fn validate_session(&self, session_id: Option<&str>) -> Result<bool, AdminError> {
        Ok(self.resolve_admin_user_id(session_id).await?.is_some())
    }

    async fn logout(&self, session_id: &str) -> Result<(), AdminError> {
        let id = session::storage_id(session_id);
        let Some(session) = self.load_live_session(id).await? else {
            return Ok(());
        };
        match (&session.renewal, session::Credential::parse(session_id)) {
            (Some(renewal), Some(credential)) if credential.owns(renewal) => {}
            (None, None) => {}
            _ => return Ok(()),
        }
        let session = self
            .store
            .delete_session(id)
            .await
            .map_err(|error| map_store_error(error, "administrator session"))?;
        if let Some(session) = session {
            let mut event = self.auth_audit("admin.logout", Utc::now());
            event.actor_admin_user_id = Some(session.admin_user_id.clone());
            event.actor_ref = crate::model::auth::admin_session_actor_ref(&session.admin_user_id);
            event.entity_ref = session.admin_user_id;
            self.store
                .append_audit_event(event)
                .await
                .map_err(|error| map_store_error(error, "administrator audit"))?;
        }
        Ok(())
    }
}

fn session_valid(session: &AdminSession) -> bool {
    let now = Utc::now();
    session.expires_at > now
        && session
            .absolute_expires_at
            .is_none_or(|expiry| expiry > now)
}

fn hash_admin_password(password: &str) -> Result<String, AdminError> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|_| AdminError::internal("管理员密码哈希失败"))
}

fn validate_new_password(password: &str) -> Result<(), AdminError> {
    let normalized = password.trim().to_ascii_lowercase();
    if password.trim().chars().count() < 12
        || password.len() > 1024
        || password.chars().any(char::is_control)
        || crate::WEAK_INITIAL_PASSWORDS.contains(&normalized.as_str())
        || matches!(
            normalized.as_str(),
            "password123456" | "123456789012" | "administrator"
        )
    {
        return Err(AdminError::invalid(
            "新密码至少需要 12 个字符，最多 1024 字节，不能使用常见弱口令或控制字符",
        ));
    }
    Ok(())
}

fn password_fingerprint(password_hash: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(password_hash.as_bytes()))
}

fn verify_admin_password(password: &str, encoded: &str) -> Result<bool, AdminError> {
    let hash = PasswordHash::new(encoded)
        .map_err(|_| AdminError::internal("已保存的管理员密码哈希不合法"))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &hash)
        .is_ok())
}

fn valid_admin_api_key_shape(value: &str) -> bool {
    value.len() == 70
        && value.starts_with("admin-")
        && value[6..].bytes().all(|byte| byte.is_ascii_hexdigit())
}
