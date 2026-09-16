use chrono::Utc;
use gateway_core::{provider_ports::ProviderUserAgentOverride, routing::ProviderKind};
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
async fn cli_user_agent_selection_round_trips_without_normalizing_text() {
    let Some(database) = TestDatabase::create("qx_user_agent").await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let repository = PgControlPlaneRepository::new(database.pool.clone());
    let runtime = PgRuntimeSettingsRepository::new(database.pool.clone());
    let before = runtime.load_runtime_settings().await.unwrap();
    for (index, user_agent) in [
        "codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color".to_owned(),
        "codex_cli_rs/0.146.0 (Linux 6.8.0; x86_64) unknown".to_owned(),
    ]
    .into_iter()
    .enumerate()
    {
        let selection = ProviderUserAgentOverride::Custom {
            user_agent: user_agent.clone(),
        };
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
            selection
        );
        let (mode, raw): (String, Option<String>) = sqlx::query_as(
            "select mode, custom_user_agent from provider_outbound_user_agents where provider_kind = 'openai'",
        ).fetch_one(&database.pool).await.unwrap();
        assert_eq!(mode, "custom");
        assert_eq!(raw.as_deref(), Some(user_agent.as_str()));
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
    let selection = ProviderUserAgentOverride::Default;
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
                    ProviderUserAgentOverride::Custom { user_agent },
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
async fn unified_selection_round_trips_without_retired_columns() {
    let Some(database) = TestDatabase::create("independent_user_agent").await else {
        return;
    };
    let provider = ProviderKind::new("openai").unwrap();
    let repository = PgControlPlaneRepository::new(database.pool.clone());
    let runtime = PgRuntimeSettingsRepository::new(database.pool.clone());
    let before = runtime.load_runtime_settings().await.unwrap();
    for (index, user_agent) in [
        None,
        Some(
            "Codex Desktop/0.153.4 (Mac OS 15.7.1; arm64) unknown (Codex Desktop; 26.901.51231)"
                .to_owned(),
        ),
        Some("codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color".to_owned()),
    ]
    .into_iter()
    .enumerate()
    {
        let selection = user_agent
            .clone()
            .map_or(ProviderUserAgentOverride::Default, |user_agent| {
                ProviderUserAgentOverride::Custom { user_agent }
            });
        repository
            .replace_user_agent_override(
                &provider,
                selection.clone(),
                audit(&format!("independent-{index}")),
            )
            .await
            .unwrap();
        let reloaded = PgControlPlaneRepository::new(database.pool.clone());
        assert_eq!(
            reloaded.load_user_agent_override(&provider).await.unwrap(),
            selection
        );
        let row: (String, Option<String>) = sqlx::query_as(
                    "select mode, custom_user_agent from provider_outbound_user_agents where provider_kind = 'openai'",
                ).fetch_one(&database.pool).await.unwrap();
        assert_eq!(
            row,
            (
                if user_agent.is_some() {
                    "custom"
                } else {
                    "default"
                }
                .to_owned(),
                user_agent.clone(),
            )
        );
    }
    for (index, selection) in [
        ProviderUserAgentOverride::Default,
        ProviderUserAgentOverride::Custom {
            user_agent: "legacy-custom".to_owned(),
        },
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
        let choices: (Option<serde_json::Value>,) = sqlx::query_as(
            "select legacy_selection from provider_outbound_user_agents where provider_kind = 'openai'",
        ).fetch_one(&database.pool).await.unwrap();
        assert_eq!(choices, (None,));
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
    let selection = ProviderUserAgentOverride::Default;
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
                    ProviderUserAgentOverride::Custom { user_agent },
                    audit("independent-invalid")
                )
                .await
                .is_err()
        );
    }
    for (mode, raw) in [
        ("independent", None),
        ("qx-compatible", None),
        ("custom", Some("")),
        ("custom", Some(" ")),
        ("default", Some("unexpected")),
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

#[tokio::test]
async fn unified_migration_archives_all_legacy_choices_and_keeps_backup_after_save() {
    let Some(database) = TestDatabase::create("unified_user_agent_upgrade").await else {
        return;
    };
    let mut transaction = database.pool.begin().await.unwrap();
    sqlx::query("drop table provider_outbound_user_agents")
        .execute(&mut *transaction)
        .await
        .unwrap();
    for migration in [
        include_str!("../../../../migrations/0009_outbound_user_agent.sql"),
        include_str!("../../../../migrations/0011_qx_compatible_user_agent.sql"),
        include_str!("../../../../migrations/0012_independent_outbound_profiles.sql"),
    ] {
        sqlx::raw_sql(migration)
            .execute(&mut *transaction)
            .await
            .unwrap();
    }
    sqlx::query(
        "insert into provider_outbound_user_agents
            (provider_kind, mode, custom_user_agent, tls_profile, session_policy)
         values ('default', 'default', null, null, null),
                ('custom', 'custom', 'exact-desktop-ua', null, null),
                ('qx-default', 'qx-compatible', null, null, null),
                ('qx-custom', 'qx-compatible', 'exact-cli-ua', null, null),
                ('independent-default', 'independent', null, 'qx-compatible', 'native'),
                ('independent-custom', 'independent', 'exact-custom-ua', 'cpr', 'qx-compatible')",
    )
    .execute(&mut *transaction)
    .await
    .unwrap();
    let before: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "select provider_kind, to_jsonb(t) - 'provider_kind'
         from provider_outbound_user_agents t order by provider_kind",
    )
    .fetch_all(&mut *transaction)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../../migrations/0018_unified_outbound_profile.sql"
    ))
    .execute(&mut *transaction)
    .await
    .unwrap();
    type MigratedSelection = (
        String,
        String,
        Option<String>,
        serde_json::Value,
        chrono::DateTime<Utc>,
    );
    let after: Vec<MigratedSelection> = sqlx::query_as(
        "select provider_kind, mode, custom_user_agent, legacy_selection, updated_at
             from provider_outbound_user_agents order by provider_kind",
    )
    .fetch_all(&mut *transaction)
    .await
    .unwrap();
    for ((id, archived), (new_id, mode, ua, backup, updated)) in before.iter().zip(&after) {
        assert_eq!(id, new_id);
        assert_eq!(
            backup, archived,
            "backup must preserve exact previous selection"
        );
        assert_eq!(
            *updated,
            chrono::DateTime::parse_from_rfc3339(archived["updated_at"].as_str().unwrap())
                .unwrap()
                .with_timezone(&Utc)
        );
        let expected = if id == "qx-default" {
            Some("codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color")
        } else {
            archived["custom_user_agent"].as_str()
        };
        assert_eq!(ua.as_deref(), expected);
        assert_eq!(
            mode,
            if expected.is_some() {
                "custom"
            } else {
                "default"
            }
        );
    }
    transaction.commit().await.unwrap();
    let repository = PgControlPlaneRepository::new(database.pool.clone());
    for (index, (id, _, _, backup, _)) in after.iter().enumerate() {
        let provider = ProviderKind::new(id).unwrap();
        for (stage, selection) in [
            ("reset", ProviderUserAgentOverride::Default),
            (
                "custom",
                ProviderUserAgentOverride::Custom {
                    user_agent: "replacement".to_owned(),
                },
            ),
        ] {
            repository
                .replace_user_agent_override(
                    &provider,
                    selection.clone(),
                    audit(&format!("unified-{index}-{stage}")),
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
            let preserved: serde_json::Value = sqlx::query_scalar(
                "select legacy_selection from provider_outbound_user_agents where provider_kind = $1",
            ).bind(id).fetch_one(&database.pool).await.unwrap();
            assert_eq!(&preserved, backup);
        }
    }
    database.close().await;
}
