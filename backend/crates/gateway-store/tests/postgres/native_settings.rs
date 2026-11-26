//! 原生平台升级保留旧值，设置与画像同事务提交，价格和密钥轮换不制造草稿冲突。
use gateway_core::{
    account::OpaqueProviderData, provider_ports::ProviderUserAgentOverride, routing::ProviderKind,
};
use gateway_store::postgres::{
    ControlPlaneRepository, PgControlPlaneRepository, PgRuntimeSnapshotRepository,
    RuntimeSnapshotRepository,
};
use serde_json::{Value, json};

use super::{TestDatabase, runtime_settings::settings_replacement};

#[tokio::test]
async fn native_upgrade_preserves_every_existing_setting_and_defaults_sharing_off() {
    let old = sqlx::migrate::Migrator {
        migrations: super::TEST_MIGRATOR
            .iter()
            .filter(|m| m.version <= 100)
            .cloned()
            .collect::<Vec<_>>()
            .into(),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    for mode in ["strict", "preferred", "relaxed"] {
        let Some(db) = TestDatabase::create_with_migrator("native_upgrade", &old).await else {
            return;
        };
        sqlx::query("update runtime_settings set config_revision=41,openai_account_affinity=$1,openai_session_binding_ttl_hours=37,refresh_margin_seconds=245,request_interval_ms=180")
            .bind(mode).execute(&db.pool).await.unwrap();
        sqlx::query("insert into client_api_keys(id,name,key,enabled,max_concurrency,requests_per_minute,created_at,updated_at) values ('key_upgrade','Upgrade fixture',$1,true,0,0,now(),now())")
            .bind(format!("sk_{}", "u".repeat(43))).execute(&db.pool).await.unwrap();
        let before: Value =
            sqlx::query_scalar("select to_jsonb(s) from runtime_settings s where id=1")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        let key_before: Value =
            sqlx::query_scalar("select to_jsonb(k) from client_api_keys k where id='key_upgrade'")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        super::TEST_MIGRATOR.run(&db.pool).await.unwrap();
        let after: Value =
            sqlx::query_scalar("select to_jsonb(s) from runtime_settings s where id=1")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        let mut expected = before;
        let fields = expected.as_object_mut().unwrap();
        let affinity = fields.remove("openai_account_affinity").unwrap();
        let ttl = fields.remove("openai_session_binding_ttl_hours").unwrap();
        fields.insert("account_affinity".into(), affinity);
        fields.insert("session_affinity_ttl_hours".into(), ttl);
        fields.insert("settings_revision".into(), json!(41));
        fields.insert(
            "codex_privacy_policy_json".into(),
            json!({"enabled":false,"onError":"skip_rule","rules":[]}),
        );
        fields.insert("pricing_overrides_json".into(), json!({}));
        fields.insert("pricing_synced_json".into(), json!({}));
        fields.insert("pricing_synced_at".into(), Value::Null);
        assert_eq!(after, expected);
        let mut key_after: Value =
            sqlx::query_scalar("select to_jsonb(k) from client_api_keys k where id='key_upgrade'")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(
            key_after
                .as_object_mut()
                .unwrap()
                .remove("peer_sharing_json"),
            Some(json!({"enabled":false,"sharePool":false,"shareQuota":false,"sharePlans":false}))
        );
        assert_eq!(key_after, key_before);
        let snapshot = PgRuntimeSnapshotRepository::new(db.pool.clone())
            .load_runtime_snapshot()
            .await
            .unwrap();
        assert_eq!(snapshot.settings.account_affinity.as_str(), mode);
        assert_eq!(snapshot.settings.session_affinity_ttl_hours, 37);
        super::TEST_MIGRATOR.run(&db.pool).await.unwrap();
        let repeated: Value =
            sqlx::query_scalar("select to_jsonb(s) from runtime_settings s where id=1")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(repeated, after);
        db.close().await;
    }
}

fn profile(marker: &str) -> OpaqueProviderData {
    OpaqueProviderData::new(json!({"marker":marker}).as_object().unwrap().clone())
}

#[tokio::test]
async fn privacy_and_three_profiles_commit_atomically_and_legacy_identity_uses_same_revision() {
    let Some(db) = TestDatabase::create("native_settings").await else {
        return;
    };
    let repository = PgControlPlaneRepository::new(db.pool.clone());
    let old = repository.load_control_plane().await.unwrap();
    let mut replacement = settings_replacement(old.settings.settings_revision, "native-save", 1200);
    replacement.settings.codex_privacy_policy = Some(
        serde_json::from_value(json!({"enabled":true,"onError":"reject_request","rules":[]}))
            .unwrap(),
    );
    for name in ["openai", "xai", "antigravity"] {
        replacement
            .settings
            .request_profile_updates
            .insert(ProviderKind::new(name).unwrap(), Some(profile(name)));
    }
    let saved = repository.replace_control_plane(replacement).await.unwrap();
    assert_eq!(
        saved.settings.settings_revision,
        saved.settings.config_revision
    );
    let snapshot = PgRuntimeSnapshotRepository::new(db.pool.clone())
        .load_runtime_snapshot()
        .await
        .unwrap();
    assert!(snapshot.settings.codex_privacy_policy.enabled);
    for name in ["openai", "xai", "antigravity"] {
        let provider = ProviderKind::new(name).unwrap();
        assert_eq!(
            snapshot.settings.request_profiles.get(&provider),
            Some(&profile(name))
        );
        assert_eq!(
            repository
                .load_user_agent_override(&provider)
                .await
                .unwrap(),
            ProviderUserAgentOverride::Configuration {
                document: profile(name)
            }
        );
    }
    assert!(
        repository
            .replace_control_plane(settings_replacement(
                old.settings.settings_revision,
                "stale-settings",
                500
            ))
            .await
            .is_err()
    );
    let draft = settings_replacement(saved.settings.settings_revision, "after-key-rotation", 1200);
    let audit = settings_replacement(saved.settings.settings_revision, "key-rotation", 1200).audit;
    repository
        .replace_admin_api_key(Some("synthetic-new-admin-credential".into()), audit)
        .await
        .unwrap();
    let after_key = repository.load_control_plane().await.unwrap();
    assert_eq!(
        after_key.settings.settings_revision,
        saved.settings.settings_revision
    );
    let saved = repository.replace_control_plane(draft).await.unwrap();
    // Omitted policy and profile changes preserve the already saved values.
    assert_eq!(
        saved.settings.codex_privacy_policy,
        snapshot.settings.codex_privacy_policy
    );
    assert_eq!(
        saved.settings.request_profiles,
        snapshot.settings.request_profiles
    );
    let stale = settings_replacement(saved.settings.settings_revision, "stale-profile", 1200);
    let audit =
        settings_replacement(saved.settings.settings_revision, "legacy-profile", 1200).audit;
    repository
        .replace_user_agent_override(
            &ProviderKind::new("openai").unwrap(),
            ProviderUserAgentOverride::CustomAutoVersion {
                user_agent: "fixture-cli/0.0.0".into(),
            },
            audit,
        )
        .await
        .unwrap();
    assert!(repository.replace_control_plane(stale).await.is_err());
    let after_legacy = repository.load_control_plane().await.unwrap();
    assert!(
        !after_legacy
            .settings
            .request_profiles
            .contains_key(&ProviderKind::new("openai").unwrap())
    );
    assert_eq!(after_legacy.settings.request_profiles.len(), 2);
    db.close().await;
}

#[tokio::test]
async fn audit_failure_rolls_back_privacy_profiles_prices_and_both_revisions() {
    use gateway_admin::model::{
        MutationActor, MutationContext,
        pricing::{PricingChange, UpdatePricing},
    };
    let Some((db, bundle)) = super::pricing::pricing_store("native_rollback").await else {
        return;
    };
    let repository = PgControlPlaneRepository::new(db.pool.clone());
    let before = repository.load_control_plane().await.unwrap();
    let settings_before: Value =
        sqlx::query_scalar("select to_jsonb(s) from runtime_settings s where id=1")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    sqlx::raw_sql("create function reject_native_audit() returns trigger language plpgsql as $$ begin raise exception 'synthetic audit failure'; end $$; create trigger reject_native_audit before insert on admin_audit_events for each row execute function reject_native_audit()")
        .execute(&db.pool).await.unwrap();
    let mut update = settings_replacement(before.settings.settings_revision, "failed-save", 1200);
    update.settings.codex_privacy_policy = Some(
        serde_json::from_value(json!({"enabled":true,"onError":"reject_request","rules":[]}))
            .unwrap(),
    );
    for name in ["openai", "xai", "antigravity"] {
        update
            .settings
            .request_profile_updates
            .insert(ProviderKind::new(name).unwrap(), Some(profile(name)));
    }
    assert!(repository.replace_control_plane(update).await.is_err());
    let settings = bundle.admin_ports().settings();
    let context = MutationContext {
        actor: MutationActor::System,
        request_id: "failed-price".into(),
    };
    assert!(settings.update_pricing(UpdatePricing { provider:"openai".into(),models:vec!["fixture-model".into()],change:PricingChange::Replace(serde_json::from_value(json!({"multiplierBps":12500,"bands":{"standard":{"input":"1","output":"2","cacheRead":"0","cacheWrite":"0"}}})).unwrap()) }, &context).await.is_err());
    let settings_after: Value =
        sqlx::query_scalar("select to_jsonb(s) from runtime_settings s where id=1")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(settings_after, settings_before);
    let profiles: i64 = sqlx::query_scalar("select count(*) from provider_outbound_user_agents")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(profiles, 0);
    let audits: i64 = sqlx::query_scalar("select count(*) from admin_audit_events")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(audits, 0);
    drop(settings);
    drop(bundle);
    drop(repository);
    db.close().await;
}
