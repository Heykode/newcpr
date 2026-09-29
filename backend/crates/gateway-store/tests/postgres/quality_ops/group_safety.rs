use super::*;
use gateway_admin::{
    model::{
        PageSize,
        accounts::{AccountGroupFilter, AccountListQuery, AccountRuntimeSnapshot, AccountStatus},
    },
    ports::store::AccountStore,
};

#[tokio::test]
async fn quality_groups_do_not_trust_forged_filters_or_config_snapshots() {
    let Some(db) = TestDatabase::create("quality_group_snapshot").await else {
        return;
    };
    let store = setup(&db).await;
    let original = store
        .save_group_rule(
            None,
            None,
            "Ungrouped".into(),
            QualityGroupFilter {
                group: "ungrouped".into(),
                statuses: vec!["normal".into()],
            },
            config(""),
            &context(),
        )
        .await
        .unwrap();
    let mut forged = original.clone();
    forged.config.model = "forged".into();
    assert!(
        store
            .apply_group_rule(
                &forged,
                &QualityTemplateTarget {
                    account_id: "acct_quality_a".into(),
                    rule_id: None,
                    revision: None,
                },
                Utc::now(),
                &context()
            )
            .await
            .is_err()
    );
    let mut forged = original.clone();
    forged.filter.group = String::new();
    assert!(
        store
            .apply_group_rule(
                &forged,
                &QualityTemplateTarget {
                    account_id: "acct_quality_a".into(),
                    rule_id: None,
                    revision: None,
                },
                Utc::now(),
                &context()
            )
            .await
            .is_err()
    );
    assert!(store.rules().await.unwrap().is_empty());
    let snapshot: serde_json::Value =
        sqlx::query_scalar("select jsonb_agg(to_jsonb(a) order by id) from provider_accounts a")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(
        store
            .apply_group_rule(
                &original,
                &QualityTemplateTarget {
                    account_id: "acct_quality_a".into(),
                    rule_id: None,
                    revision: None,
                },
                Utc::now(),
                &context()
            )
            .await
            .unwrap()
    );
    let after: serde_json::Value =
        sqlx::query_scalar("select jsonb_agg(to_jsonb(a) order by id) from provider_accounts a")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(snapshot, after);
    db.close().await;
}

#[tokio::test]
async fn quality_group_pause_survives_deleted_template_and_reports_resume_conflict() {
    let Some(db) = TestDatabase::create("quality_group_pause_template").await else {
        return;
    };
    let store = setup(&db).await;
    let template: gateway_admin::model::relogin_templates::ReloginTemplate =
        serde_json::from_value(serde_json::json!({
            "id":"group-template", "revision":1,
            "config":{"name":"Group template","enabled":true,"concurrencyLimit":5,"weight":20,
                "groupIds":[],"outboundProxyId":null,"egressMode":"unchanged"}
        }))
        .unwrap();
    sqlx::query(
        "insert into account_relogin_templates(id,revision,name,config) values($1,1,$2,$3)",
    )
    .bind(&template.id)
    .bind(&template.config.name)
    .bind(serde_json::to_value(&template.config).unwrap())
    .execute(&db.pool)
    .await
    .unwrap();
    let mut cfg = config("");
    cfg.failure_action = QualityFailureAction::ApplyAccountTemplate;
    cfg.failure_template = Some(template);
    let group = store
        .save_group_rule(
            None,
            None,
            "Paused group".into(),
            QualityGroupFilter::default(),
            cfg,
            &context(),
        )
        .await
        .unwrap();
    let target = QualityTemplateTarget {
        account_id: "acct_quality_a".into(),
        rule_id: None,
        revision: None,
    };
    assert!(
        store
            .apply_group_rule(&group, &target, Utc::now(), &context())
            .await
            .unwrap()
    );
    let claim = store.claim().await.unwrap().unwrap();
    sqlx::query("delete from account_relogin_templates")
        .execute(&db.pool)
        .await
        .unwrap();
    let mut paused_config = group.config.clone();
    paused_config.enabled = false;
    let paused = store
        .save_group_rule(
            Some(&group.id),
            Some(group.revision),
            group.name,
            group.filter,
            paused_config,
            &context(),
        )
        .await
        .unwrap();
    assert!(!store.current(&claim).await.unwrap());
    assert!(!store.rules().await.unwrap()[0].config.enabled);
    let pending = store.group_targets(&paused.id, "").await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(
        store
            .apply_group_rule(&paused, &pending[0], Utc::now(), &context())
            .await
            .is_err()
    );
    assert_eq!(store.group_targets(&paused.id, "").await.unwrap().len(), 1);
    let mut resumed = paused.config.clone();
    resumed.enabled = true;
    assert!(
        store
            .save_group_rule(
                Some(&paused.id),
                Some(paused.revision),
                paused.name,
                paused.filter,
                resumed,
                &context()
            )
            .await
            .is_err()
    );
    assert!(!store.group_rules().await.unwrap()[0].config.enabled);
    db.close().await;
}

#[tokio::test]
async fn quality_group_owned_updates_page_past_one_hundred_without_duplicate_updates() {
    let Some(db) = TestDatabase::create("quality_group_pages").await else {
        return;
    };
    let store = setup(&db).await;
    sqlx::query("insert into provider_accounts(id,provider_kind,name,upstream_user_id,
        authentication_kind,provider_credentials_json,has_refresh_token,credential_observed_at,
        created_at,updated_at,credential_state)
        select 'acct_page_'||n,'openai','page '||n,'page '||n,'oauth','{}',false,now(),now(),now(),'ready'
        from generate_series(1,105) n").execute(&db.pool).await.unwrap();
    let mut group = store
        .save_group_rule(
            None,
            None,
            "Paged group".into(),
            QualityGroupFilter::default(),
            config(""),
            &context(),
        )
        .await
        .unwrap();
    for n in 1..=105 {
        assert!(
            store
                .apply_group_rule(
                    &group,
                    &QualityTemplateTarget {
                        account_id: format!("acct_page_{n}"),
                        rule_id: None,
                        revision: None,
                    },
                    Utc::now() + Duration::hours(1),
                    &context()
                )
                .await
                .unwrap()
        );
    }
    group.config.interval_seconds = Some(60);
    group = store
        .save_group_rule(
            Some(&group.id),
            Some(group.revision),
            group.name,
            group.filter,
            group.config,
            &context(),
        )
        .await
        .unwrap();
    let first = store.group_targets(&group.id, "").await.unwrap();
    assert_eq!(first.len(), 100);
    let second = store
        .group_targets(&group.id, &first.last().unwrap().account_id)
        .await
        .unwrap();
    assert_eq!(second.len(), 5);
    for target in first.iter().chain(&second) {
        assert!(
            store
                .apply_group_rule(
                    &group,
                    target,
                    Utc::now() + Duration::seconds(60),
                    &context()
                )
                .await
                .unwrap()
        );
    }
    assert!(store.group_targets(&group.id, "").await.unwrap().is_empty());
    assert_eq!(store.group_rules().await.unwrap()[0].rule_count, 105);
    db.close().await;
}

#[tokio::test]
async fn quality_group_candidates_follow_real_account_group_and_status_transitions() {
    let Some(db) = TestDatabase::create("quality_group_candidates").await else {
        return;
    };
    let store = setup(&db).await;
    let accounts = super::super::admin_account_store(&db.pool);
    let group_id = "grp_00000000000000000000000000000001";
    sqlx::query("insert into account_groups(id,name,color,created_at,updated_at) values($1,'Candidates','#FFFFFFFF',now(),now())")
        .bind(group_id).execute(&db.pool).await.unwrap();
    let group = store
        .save_group_rule(
            None,
            None,
            "Normal only".into(),
            QualityGroupFilter {
                group: group_id.into(),
                statuses: vec!["normal".into()],
            },
            config(""),
            &context(),
        )
        .await
        .unwrap();
    let query = AccountListQuery {
        page: 1,
        page_size: PageSize::new(100).unwrap(),
        provider_kind: Some(gateway_core::routing::ProviderKind::new("openai").unwrap()),
        group_filter: Some(AccountGroupFilter::Group(
            gateway_core::routing::AccountGroupId::new(group_id).unwrap(),
        )),
        status: Some(AccountStatus::Normal),
        search: None,
        plan_type: None,
        sort: None,
    };
    assert!(
        accounts
            .list_accounts(query.clone(), AccountRuntimeSnapshot::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    sqlx::query("insert into account_group_accounts(account_group_id,provider_account_id,created_at)
        select $1,id,now() from provider_accounts where id in ('acct_quality_a','acct_quality_b','acct_quality_c')")
        .bind(group_id).execute(&db.pool).await.unwrap();
    sqlx::query(
        "update provider_accounts set credential_state='expired' where id='acct_quality_b'",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("update provider_accounts set enabled=false where id='acct_quality_c'")
        .execute(&db.pool)
        .await
        .unwrap();
    for expected in [vec!["acct_quality_a"], vec!["acct_quality_b"]] {
        let page = accounts
            .list_accounts(query.clone(), AccountRuntimeSnapshot::default())
            .await
            .unwrap();
        let new: Vec<_> = page
            .items
            .into_iter()
            .filter(|item| item.account.quality_monitoring.is_none())
            .collect();
        assert_eq!(
            new.iter()
                .map(|item| item.account.id.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        for item in new {
            assert!(
                store
                    .apply_group_rule(
                        &group,
                        &QualityTemplateTarget {
                            account_id: item.account.id,
                            rule_id: None,
                            revision: None,
                        },
                        Utc::now() + Duration::hours(1),
                        &context()
                    )
                    .await
                    .unwrap()
            );
        }
        sqlx::query(
            "update provider_accounts set credential_state='ready' where id='acct_quality_b'",
        )
        .execute(&db.pool)
        .await
        .unwrap();
    }
    assert_eq!(store.rules().await.unwrap().len(), 2);
    assert!(
        store
            .rules()
            .await
            .unwrap()
            .iter()
            .all(|r| !r.pending && !r.running)
    );
    db.close().await;
}
