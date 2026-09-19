use chrono::Utc;
use gateway_store::postgres::{AdminAuditActorKind, AdminAuditEvent};

#[test]
fn audit_event_rejects_more_than_sixty_four_changed_fields() {
    let event = AdminAuditEvent {
        id: "audit-1".to_owned(),
        actor_kind: AdminAuditActorKind::System,
        actor_admin_user_id: None,
        actor_ref: "system".to_owned(),
        admin_request_id: None,
        action: "update".to_owned(),
        entity_kind: "settings".to_owned(),
        entity_ref: "1".to_owned(),
        config_revision: Some(2),
        changed_fields: (0..65).map(|index| format!("field-{index}")).collect(),
        created_at: Utc::now(),
    };
    assert!(event.validate().is_err());
}

#[tokio::test]
async fn password_change_is_compare_and_swap_and_audit_failure_rolls_back() {
    use gateway_store::postgres::{
        AdminSecurityAuditRepository as _, PgAdminSecurityAuditRepository,
    };
    let Some(database) = super::TestDatabase::create("password_change").await else {
        return;
    };
    let repository = PgAdminSecurityAuditRepository::new(database.pool.clone());
    repository
        .create_password_hash_if_absent("admin-test", "synthetic-original-hash")
        .await
        .expect("create");
    let event = AdminAuditEvent {
        id: "audit-password-change".to_owned(),
        actor_kind: AdminAuditActorKind::AdminSession,
        actor_admin_user_id: Some("admin-test".to_owned()),
        actor_ref: "admin:admin-test".to_owned(),
        admin_request_id: None,
        action: "admin.password_changed".to_owned(),
        entity_kind: "admin_user".to_owned(),
        entity_ref: "admin-test".to_owned(),
        config_revision: None,
        changed_fields: vec!["password".to_owned()],
        created_at: Utc::now(),
    };
    assert!(
        !repository
            .change_password(
                "admin-test",
                "stale-hash",
                "synthetic-new-hash",
                event.clone()
            )
            .await
            .expect("stale")
    );
    assert!(
        repository
            .change_password(
                "admin-test",
                "synthetic-original-hash",
                "synthetic-new-hash",
                event.clone()
            )
            .await
            .expect("change")
    );
    assert!(
        repository
            .change_password(
                "admin-test",
                "synthetic-new-hash",
                "must-roll-back-hash",
                event
            )
            .await
            .is_err(),
        "duplicate audit ID aborts transaction"
    );
    assert_eq!(
        repository
            .password_hash("admin-test")
            .await
            .expect("hash")
            .as_deref(),
        Some("synthetic-new-hash")
    );
    let audits: i64 = sqlx::query_scalar("select count(*) from admin_audit_events")
        .fetch_one(&database.pool)
        .await
        .expect("audits");
    assert_eq!(audits, 1);
    database.close().await;
}
