use chrono::Utc;
use gateway_core::{
    provider_ports::{ProviderSessionPolicy, ProviderTlsProfile, ProviderUserAgentOverride},
    routing::ProviderKind,
};
use gateway_store::postgres::{
    AdminAuditActorKind, AdminAuditEvent, PgControlPlaneRepository, PgRuntimeSettingsRepository,
    RuntimeSettingsRepository,
};

use super::TestDatabase;

#[tokio::test]
async fn targeted_user_agent_save_survives_reload_and_preserves_runtime_settings() {
    let Some(database) = TestDatabase::create("outbound_user_agent").await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let repository = PgControlPlaneRepository::new(database.pool.clone());
    let runtime = PgRuntimeSettingsRepository::new(database.pool.clone());
    let before = runtime.load_runtime_settings().await.unwrap();
    assert_eq!(
        repository
            .load_user_agent_override(&provider)
            .await
            .unwrap(),
        ProviderUserAgentOverride::Default
    );
    let custom = ProviderUserAgentOverride::Custom {
        user_agent:
            "Codex Desktop/0.153.4 (Mac OS 15.7.1; arm64) unknown (Codex Desktop; 26.901.51231)"
                .to_owned(),
    };
    repository
        .replace_user_agent_override(&provider, custom.clone(), audit("ua-custom"))
        .await
        .unwrap();
    let reloaded = PgControlPlaneRepository::new(database.pool.clone());
    assert_eq!(
        reloaded.load_user_agent_override(&provider).await.unwrap(),
        custom
    );
    let after = runtime.load_runtime_settings().await.unwrap();
    assert_eq!(
        after.max_concurrent_per_account,
        before.max_concurrent_per_account
    );
    assert_eq!(after.rotation_strategy, before.rotation_strategy);
    assert_eq!(after.model_mappings, before.model_mappings);
    assert_eq!(after.request_tuning, before.request_tuning);
    assert_eq!(after.admin_api_key, before.admin_api_key);
    assert!(after.config_revision > before.config_revision);
    repository
        .replace_user_agent_override(
            &provider,
            ProviderUserAgentOverride::Default,
            audit("ua-default"),
        )
        .await
        .unwrap();
    assert_eq!(
        reloaded.load_user_agent_override(&provider).await.unwrap(),
        ProviderUserAgentOverride::Default
    );
    database.close().await;
}

fn audit(id: &str) -> AdminAuditEvent {
    AdminAuditEvent {
        id: id.to_owned(),
        actor_kind: AdminAuditActorKind::System,
        actor_admin_user_id: None,
        actor_ref: "system".to_owned(),
        admin_request_id: Some(id.to_owned()),
        action: "outbound_user_agent.replace".to_owned(),
        entity_kind: "provider_outbound_user_agents".to_owned(),
        entity_ref: "openai".to_owned(),
        config_revision: None,
        changed_fields: vec!["mode".to_owned()],
        created_at: Utc::now(),
    }
}

#[tokio::test]
async fn qx_user_agent_selection_round_trips_and_blank_uses_null() {
    let Some(database) = TestDatabase::create("qx_user_agent").await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let repository = PgControlPlaneRepository::new(database.pool.clone());
    let runtime = PgRuntimeSettingsRepository::new(database.pool.clone());
    let before = runtime.load_runtime_settings().await.unwrap();
    for (index, user_agent) in [
        None,
        Some("codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color".to_owned()),
        Some("codex_cli_rs/0.146.0 (Linux 6.8.0; x86_64) unknown".to_owned()),
        Some("   ".to_owned()),
        Some(String::new()),
    ]
    .into_iter()
    .enumerate()
    {
        let selection = ProviderUserAgentOverride::QxCompatible { user_agent };
        repository
            .replace_user_agent_override(
                &provider,
                selection.clone(),
                audit(&format!("qx-{index}")),
            )
            .await
            .unwrap();
        let reloaded = PgControlPlaneRepository::new(database.pool.clone());
        assert_eq!(
            reloaded.load_user_agent_override(&provider).await.unwrap(),
            selection.normalized()
        );
        let (mode, raw): (String, Option<String>) = sqlx::query_as(
            "select mode, custom_user_agent from provider_outbound_user_agents where provider_kind = 'openai'",
        ).fetch_one(&database.pool).await.unwrap();
        assert_eq!(mode, "qx-compatible");
        if index >= 3 {
            assert_eq!(raw, None);
        }
    }
    let after = runtime.load_runtime_settings().await.unwrap();
    assert_eq!(after.request_tuning, before.request_tuning);
    assert_eq!(after.model_mappings, before.model_mappings);
    assert_eq!(
        after.max_concurrent_per_account,
        before.max_concurrent_per_account
    );
    assert_eq!(after.admin_api_key, before.admin_api_key);
    assert!(after.config_revision > before.config_revision);
    repository
        .replace_user_agent_override(
            &provider,
            ProviderUserAgentOverride::Default,
            audit("qx-reset"),
        )
        .await
        .unwrap();
    assert_eq!(
        repository
            .load_user_agent_override(&provider)
            .await
            .unwrap(),
        ProviderUserAgentOverride::Default
    );
    database.close().await;
}

#[tokio::test]
async fn qx_invalid_persistence_does_not_mutate_selection_or_revision() {
    let Some(database) = TestDatabase::create("qx_user_agent_invalid").await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let repository = PgControlPlaneRepository::new(database.pool.clone());
    let runtime = PgRuntimeSettingsRepository::new(database.pool.clone());
    let selection = ProviderUserAgentOverride::QxCompatible { user_agent: None };
    repository
        .replace_user_agent_override(&provider, selection.clone(), audit("qx-initial"))
        .await
        .unwrap();
    let before = runtime.load_runtime_settings().await.unwrap();
    for user_agent in [
        "ua\r\nInjected: true".to_owned(),
        "\t".to_owned(),
        "x".repeat(513),
        "非ASCII".to_owned(),
    ] {
        assert!(
            repository
                .replace_user_agent_override(
                    &provider,
                    ProviderUserAgentOverride::QxCompatible {
                        user_agent: Some(user_agent)
                    },
                    audit("qx-invalid"),
                )
                .await
                .is_err()
        );
        assert_eq!(
            repository
                .load_user_agent_override(&provider)
                .await
                .unwrap(),
            selection
        );
        assert_eq!(
            runtime
                .load_runtime_settings()
                .await
                .unwrap()
                .config_revision,
            before.config_revision
        );
    }
    for (mode, raw) in [
        ("guess", None),
        ("default", Some("unexpected")),
        ("custom", None),
        ("qx-compatible", Some("   ")),
        ("qx-compatible", Some("unsafe\n")),
    ] {
        assert!(sqlx::query(
            "update provider_outbound_user_agents set mode = $1, custom_user_agent = $2 where provider_kind = 'openai'",
        ).bind(mode).bind(raw).execute(&database.pool).await.is_err());
    }
    assert_eq!(
        repository
            .load_user_agent_override(&provider)
            .await
            .unwrap(),
        selection
    );
    database.close().await;
}

#[tokio::test]
async fn qx_migration_preserves_existing_default_and_custom_rows() {
    let Some(database) = TestDatabase::create("qx_user_agent_upgrade").await else {
        return;
    };
    let mut transaction = database.pool.begin().await.unwrap();
    // Recreate the frozen 0009 table inside this isolated schema/transaction.
    sqlx::query("drop table provider_outbound_user_agents")
        .execute(&mut *transaction)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../../migrations/0009_outbound_user_agent.sql"
    ))
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "insert into provider_outbound_user_agents (provider_kind, mode, custom_user_agent)
         values ('default-fixture', 'default', null), ('custom-fixture', 'custom', 'same-default-text')",
    )
    .execute(&mut *transaction)
    .await
    .unwrap();
    let before: Vec<(String, String, Option<String>, chrono::DateTime<Utc>)> = sqlx::query_as(
        "select provider_kind, mode, custom_user_agent, updated_at
         from provider_outbound_user_agents order by provider_kind",
    )
    .fetch_all(&mut *transaction)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../../migrations/0011_qx_compatible_user_agent.sql"
    ))
    .execute(&mut *transaction)
    .await
    .unwrap();
    let after: Vec<(String, String, Option<String>, chrono::DateTime<Utc>)> = sqlx::query_as(
        "select provider_kind, mode, custom_user_agent, updated_at
         from provider_outbound_user_agents order by provider_kind",
    )
    .fetch_all(&mut *transaction)
    .await
    .unwrap();
    assert_eq!(after, before);
    sqlx::query(
        "insert into provider_outbound_user_agents (provider_kind, mode)
         values ('qx-fixture', 'qx-compatible')",
    )
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.rollback().await.unwrap();
    database.close().await;
}

#[tokio::test]
async fn independent_selection_round_trips_all_choices_and_resets_to_legacy_without_stale_columns()
{
    let Some(database) = TestDatabase::create("independent_user_agent").await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let repository = PgControlPlaneRepository::new(database.pool.clone());
    let runtime = PgRuntimeSettingsRepository::new(database.pool.clone());
    let before = runtime.load_runtime_settings().await.unwrap();
    let mut index = 0;
    for user_agent in [
        None,
        Some(
            "Codex Desktop/0.153.4 (Mac OS 15.7.1; arm64) unknown (Codex Desktop; 26.901.51231)"
                .to_owned(),
        ),
        Some("codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color".to_owned()),
    ] {
        for tls_profile in [ProviderTlsProfile::Cpr, ProviderTlsProfile::QxCompatible] {
            for session_policy in [
                ProviderSessionPolicy::Native,
                ProviderSessionPolicy::QxCompatible,
            ] {
                let selection = ProviderUserAgentOverride::Independent {
                    user_agent: user_agent.clone(),
                    tls_profile,
                    session_policy,
                };
                repository
                    .replace_user_agent_override(
                        &provider,
                        selection.clone(),
                        audit(&format!("independent-{index}")),
                    )
                    .await
                    .unwrap();
                index += 1;
                let reloaded = PgControlPlaneRepository::new(database.pool.clone());
                assert_eq!(
                    reloaded.load_user_agent_override(&provider).await.unwrap(),
                    selection
                );
                let row: (String, Option<String>, String, String) = sqlx::query_as(
                    "select mode, custom_user_agent, tls_profile, session_policy from provider_outbound_user_agents where provider_kind = 'openai'",
                ).fetch_one(&database.pool).await.unwrap();
                assert_eq!(
                    row,
                    (
                        "independent".to_owned(),
                        user_agent.clone(),
                        tls_profile.as_str().to_owned(),
                        session_policy.as_str().to_owned()
                    )
                );
            }
        }
    }
    for (index, selection) in [
        ProviderUserAgentOverride::Default,
        ProviderUserAgentOverride::Custom {
            user_agent: "legacy-custom".to_owned(),
        },
        ProviderUserAgentOverride::QxCompatible { user_agent: None },
    ]
    .into_iter()
    .enumerate()
    {
        repository
            .replace_user_agent_override(
                &provider,
                selection.clone(),
                audit(&format!("legacy-{index}")),
            )
            .await
            .unwrap();
        assert_eq!(
            repository
                .load_user_agent_override(&provider)
                .await
                .unwrap(),
            selection
        );
        let choices: (Option<String>, Option<String>) = sqlx::query_as(
            "select tls_profile, session_policy from provider_outbound_user_agents where provider_kind = 'openai'",
        ).fetch_one(&database.pool).await.unwrap();
        assert_eq!(choices, (None, None));
    }
    let after = runtime.load_runtime_settings().await.unwrap();
    assert_eq!(after.request_tuning, before.request_tuning);
    assert_eq!(after.model_mappings, before.model_mappings);
    assert_eq!(after.admin_api_key, before.admin_api_key);
    assert!(after.config_revision > before.config_revision);
    database.close().await;
}

#[tokio::test]
async fn independent_invalid_choices_and_blank_custom_do_not_commit() {
    let Some(database) = TestDatabase::create("independent_invalid_user_agent").await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let repository = PgControlPlaneRepository::new(database.pool.clone());
    let runtime = PgRuntimeSettingsRepository::new(database.pool.clone());
    let selection = ProviderUserAgentOverride::Independent {
        user_agent: None,
        tls_profile: ProviderTlsProfile::QxCompatible,
        session_policy: ProviderSessionPolicy::Native,
    };
    repository
        .replace_user_agent_override(&provider, selection.clone(), audit("independent-valid"))
        .await
        .unwrap();
    let before = runtime.load_runtime_settings().await.unwrap();
    for user_agent in [
        "".to_owned(),
        "   ".to_owned(),
        "\r\n".to_owned(),
        "x".repeat(513),
    ] {
        assert!(
            repository
                .replace_user_agent_override(
                    &provider,
                    ProviderUserAgentOverride::Independent {
                        user_agent: Some(user_agent),
                        tls_profile: ProviderTlsProfile::Cpr,
                        session_policy: ProviderSessionPolicy::QxCompatible,
                    },
                    audit("independent-invalid")
                )
                .await
                .is_err()
        );
    }
    for (mode, raw, tls, session) in [
        ("independent", None, None, Some("native")),
        ("independent", None, Some("cpr"), None),
        ("independent", None, Some("guess"), Some("native")),
        ("independent", None, Some("cpr"), Some("guess")),
        ("independent", Some(""), Some("cpr"), Some("native")),
        ("independent", Some(" "), Some("cpr"), Some("native")),
        ("default", None, Some("cpr"), Some("native")),
        ("qx-compatible", None, None, Some("qx-compatible")),
    ] {
        assert!(sqlx::query(
            "update provider_outbound_user_agents set mode = $1, custom_user_agent = $2, tls_profile = $3, session_policy = $4 where provider_kind = 'openai'",
        ).bind(mode).bind(raw).bind(tls).bind(session).execute(&database.pool).await.is_err());
    }
    assert_eq!(
        repository
            .load_user_agent_override(&provider)
            .await
            .unwrap(),
        selection
    );
    assert_eq!(
        runtime
            .load_runtime_settings()
            .await
            .unwrap()
            .config_revision,
        before.config_revision
    );
    database.close().await;
}

#[tokio::test]
async fn independent_migration_preserves_legacy_values_and_timestamps() {
    let Some(database) = TestDatabase::create("independent_user_agent_upgrade").await else {
        return;
    };
    let mut transaction = database.pool.begin().await.unwrap();
    sqlx::query("drop table provider_outbound_user_agents")
        .execute(&mut *transaction)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../../migrations/0009_outbound_user_agent.sql"
    ))
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../../migrations/0011_qx_compatible_user_agent.sql"
    ))
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "insert into provider_outbound_user_agents (provider_kind, mode, custom_user_agent)
         values ('default-fixture', 'default', null), ('custom-fixture', 'custom', 'same-default-text'),
                ('qx-fixture', 'qx-compatible', null), ('qx-custom-fixture', 'qx-compatible', 'cli-text')",
    ).execute(&mut *transaction).await.unwrap();
    let before: Vec<(String, String, Option<String>, chrono::DateTime<Utc>)> = sqlx::query_as(
        "select provider_kind, mode, custom_user_agent, updated_at from provider_outbound_user_agents order by provider_kind",
    ).fetch_all(&mut *transaction).await.unwrap();
    sqlx::raw_sql(include_str!(
        "../../../../migrations/0012_independent_outbound_profiles.sql"
    ))
    .execute(&mut *transaction)
    .await
    .unwrap();
    let after: Vec<(String, String, Option<String>, chrono::DateTime<Utc>)> = sqlx::query_as(
        "select provider_kind, mode, custom_user_agent, updated_at from provider_outbound_user_agents order by provider_kind",
    ).fetch_all(&mut *transaction).await.unwrap();
    assert_eq!(before, after);
    let count: i64 = sqlx::query_scalar(
        "select count(*) from provider_outbound_user_agents where tls_profile is null and session_policy is null",
    ).fetch_one(&mut *transaction).await.unwrap();
    assert_eq!(count, 4);
    transaction.rollback().await.unwrap();
    database.close().await;
}
