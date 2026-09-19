use std::{collections::BTreeMap, sync::Mutex};

use async_trait::async_trait;
use chrono::{TimeDelta, Utc};

use gateway_admin::{
    model::{
        auth::{AdminAuditEvent, AdminSession, ChangePassword, LoginCommand},
        settings::AdminApiKey,
    },
    ports::store::{AdminStoreResult, AuthStore},
};

#[derive(Default)]
struct MemoryAuthStore {
    password_hash: Mutex<Option<String>>,
    sessions: Mutex<BTreeMap<String, AdminSession>>,
    audits: Mutex<Vec<AdminAuditEvent>>,
    attempts: Mutex<u32>,
    fail_change: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl AuthStore for MemoryAuthStore {
    async fn change_password(
        &self,
        _: &str,
        expected_hash: &str,
        password_hash: &str,
        audit: AdminAuditEvent,
    ) -> AdminStoreResult<bool> {
        let mut stored = self.password_hash.lock().expect("hash");
        if self.fail_change.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(super::unavailable("password transaction"));
        }
        if stored.as_deref() != Some(expected_hash) {
            return Ok(false);
        }
        *stored = Some(password_hash.to_owned());
        self.audits.lock().expect("audit").push(audit);
        Ok(true)
    }

    async fn consume_password_change_attempt(
        &self,
        _: &str,
        limit: u32,
        _: u64,
    ) -> AdminStoreResult<bool> {
        let mut attempts = self.attempts.lock().expect("attempts");
        *attempts += 1;
        Ok(*attempts <= limit)
    }

    async fn load_password_hash(&self, _: &str) -> AdminStoreResult<Option<String>> {
        Ok(self.password_hash.lock().expect("password hash").clone())
    }

    async fn create_password_hash_if_absent(
        &self,
        _: &str,
        password_hash: &str,
    ) -> AdminStoreResult<bool> {
        let mut stored = self.password_hash.lock().expect("password hash");
        if stored.is_some() {
            return Ok(false);
        }
        *stored = Some(password_hash.to_owned());
        Ok(true)
    }

    async fn load_admin_api_key(&self) -> AdminStoreResult<Option<AdminApiKey>> {
        Ok(None)
    }

    async fn load_session(&self, session_id: &str) -> AdminStoreResult<Option<AdminSession>> {
        Ok(self
            .sessions
            .lock()
            .expect("sessions")
            .get(session_id)
            .cloned())
    }

    async fn store_session(
        &self,
        session_id: &str,
        session: &AdminSession,
    ) -> AdminStoreResult<()> {
        self.sessions
            .lock()
            .expect("sessions")
            .insert(session_id.to_owned(), session.clone());
        Ok(())
    }

    async fn delete_session(&self, session_id: &str) -> AdminStoreResult<Option<AdminSession>> {
        Ok(self.sessions.lock().expect("sessions").remove(session_id))
    }

    async fn append_audit_event(&self, event: AdminAuditEvent) -> AdminStoreResult<()> {
        self.audits.lock().expect("audits").push(event);
        Ok(())
    }
}

#[tokio::test]
async fn successful_login_should_create_expiring_session_and_audit() {
    let store = std::sync::Arc::new(MemoryAuthStore::default());
    let services = super::AdminHarness::new().auth(store.clone()).build().await;

    let result = services
        .auth()
        .login(LoginCommand {
            username: Some("admin".to_owned()),
            password: "strong-test-password".to_owned(),
        })
        .await
        .expect("login");

    assert!(
        services
            .auth()
            .validate_session(Some(&result.session_id))
            .await
            .expect("validate")
    );
    assert_eq!(store.audits.lock().expect("audits").len(), 1);
}

#[tokio::test]
async fn repeated_default_initialization_should_not_replace_password() {
    let store = std::sync::Arc::new(MemoryAuthStore::default());
    super::AdminHarness::new()
        .auth(store.clone())
        .default_password("first-strong-password")
        .build()
        .await;
    let services = super::AdminHarness::new()
        .auth(store)
        .default_password("second-strong-password")
        .build()
        .await;

    assert!(
        services
            .auth()
            .login(LoginCommand {
                username: Some("admin".to_owned()),
                password: "first-strong-password".to_owned(),
            })
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn login_with_huge_session_ttl_should_clamp_expiry_instead_of_panicking() {
    let store = std::sync::Arc::new(MemoryAuthStore::default());
    let services = super::AdminHarness::new()
        .auth(store.clone())
        .session_ttl_minutes(i64::MAX as u64)
        .build()
        .await;

    let before = Utc::now();
    let result = services
        .auth()
        .login(LoginCommand {
            username: Some("admin".to_owned()),
            password: "strong-test-password".to_owned(),
        })
        .await
        .expect("login with huge session TTL");

    assert!(result.expires_at > before);
    assert!(result.expires_at <= before + TimeDelta::days(366) + TimeDelta::minutes(1));
    assert!(
        services
            .auth()
            .validate_session(Some(&result.session_id))
            .await
            .expect("validate")
    );
}

#[tokio::test]
async fn password_change_revokes_all_sessions_and_keeps_admin_api_key_separate() {
    let store = std::sync::Arc::new(MemoryAuthStore::default());
    let services = super::AdminHarness::new().auth(store.clone()).build().await;
    let login = LoginCommand {
        username: None,
        password: "strong-test-password".to_owned(),
    };
    let first = services.auth().login(login.clone()).await.expect("login");
    let second = services
        .auth()
        .login(login.clone())
        .await
        .expect("second login");
    services
        .auth()
        .change_password(
            Some(&first.session_id),
            ChangePassword {
                current_password: login.password.clone(),
                new_password: "replacement-test-password".to_owned(),
            },
        )
        .await
        .expect("change password");
    for session in [&first, &second] {
        assert!(
            !services
                .auth()
                .validate_session(Some(&session.session_id))
                .await
                .expect("validate")
        );
    }
    assert_eq!(
        store.sessions.lock().expect("sessions").len(),
        2,
        "revocation does not require deleting Redis records"
    );
    assert!(services.auth().login(login).await.is_err());
    assert!(
        services
            .auth()
            .login(LoginCommand {
                username: None,
                password: "replacement-test-password".to_owned(),
            })
            .await
            .is_ok()
    );
    let audits = store.audits.lock().expect("audits");
    assert_eq!(
        audits
            .iter()
            .filter(|event| event.action == "admin.password_changed")
            .count(),
        1
    );
    assert!(!format!("{audits:?}").contains("replacement-test-password"));
}

#[tokio::test]
async fn password_change_rejects_invalid_passwords_missing_session_and_failed_transaction() {
    let store = std::sync::Arc::new(MemoryAuthStore::default());
    let services = super::AdminHarness::new().auth(store.clone()).build().await;
    let login = LoginCommand {
        username: None,
        password: "strong-test-password".to_owned(),
    };
    let session = services.auth().login(login.clone()).await.expect("login");
    let command = |current: &str, next: &str| ChangePassword {
        current_password: current.to_owned(),
        new_password: next.to_owned(),
    };
    assert!(
        services
            .auth()
            .change_password(None, command(&login.password, "replacement-test-password"))
            .await
            .is_err()
    );
    for next in [
        "short",
        "            ",
        "strong-test-password",
        "password123456",
        "newline-password\n",
    ] {
        assert!(
            services
                .auth()
                .change_password(Some(&session.session_id), command(&login.password, next))
                .await
                .is_err()
        );
    }
    assert!(
        services
            .auth()
            .change_password(
                Some(&session.session_id),
                command("incorrect", "replacement-test-password")
            )
            .await
            .is_err()
    );
    store
        .fail_change
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        services
            .auth()
            .change_password(
                Some(&session.session_id),
                command(&login.password, "replacement-test-password")
            )
            .await
            .is_err()
    );
    assert!(
        services
            .auth()
            .validate_session(Some(&session.session_id))
            .await
            .expect("old session preserved")
    );
    assert!(services.auth().login(login).await.is_ok());
    assert!(
        !store
            .audits
            .lock()
            .expect("audit")
            .iter()
            .any(|event| event.action == "admin.password_changed")
    );
}

#[tokio::test]
async fn password_change_attempt_limit_is_shared_across_sessions_and_legacy_sessions_fail_closed() {
    let store = std::sync::Arc::new(MemoryAuthStore::default());
    let services = super::AdminHarness::new().auth(store.clone()).build().await;
    let login = LoginCommand {
        username: None,
        password: "strong-test-password".to_owned(),
    };
    let first = services.auth().login(login.clone()).await.expect("login");
    let second = services.auth().login(login).await.expect("login");
    for attempt in 0..11 {
        let session = if attempt % 2 == 0 { &first } else { &second };
        let error = services
            .auth()
            .change_password(
                Some(&session.session_id),
                ChangePassword {
                    current_password: "wrong".to_owned(),
                    new_password: "replacement-test-password".to_owned(),
                },
            )
            .await
            .expect_err("invalid");
        assert_eq!(
            error.kind(),
            if attempt < 10 {
                gateway_admin::model::AdminErrorKind::Invalid
            } else {
                gateway_admin::model::AdminErrorKind::RateLimited
            }
        );
    }
    store
        .sessions
        .lock()
        .expect("sessions")
        .get_mut(&first.session_id)
        .expect("first")
        .credential_fingerprint
        .clear();
    assert!(
        !services
            .auth()
            .validate_session(Some(&first.session_id))
            .await
            .expect("legacy session")
    );
}
