use super::*;

fn target(account: &str, rule: Option<&QualityRule>) -> QualityTemplateTarget {
    QualityTemplateTarget {
        account_id: account.into(),
        rule_id: rule.map(|rule| rule.id.clone()),
        revision: rule.map(|rule| rule.revision),
    }
}

async fn create_group(store: &PgQualityOpsStore, filter: QualityGroupFilter) -> QualityGroupRule {
    store
        .save_group_rule(
            None,
            None,
            "Fixture group".into(),
            filter,
            config(""),
            &context(),
        )
        .await
        .unwrap()
}

async fn apply(store: &PgQualityOpsStore, group: &QualityGroupRule, account: &str) -> bool {
    store
        .apply_group_rule(
            group,
            &target(account, None),
            Utc::now() + Duration::hours(1),
            &context(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn quality_groups_fence_concurrency_independent_rules_and_manual_deletion() {
    let Some(db) = TestDatabase::create("quality_group_owners").await else {
        return;
    };
    let store = setup(&db).await;
    let group = create_group(&store, QualityGroupFilter::default()).await;
    let other = create_group(&store, QualityGroupFilter::default()).await;
    let (first, second) = tokio::join!(
        apply(&store, &group, "acct_quality_a"),
        apply(&store, &other, "acct_quality_a")
    );
    assert_ne!(first, second);
    let owner = if first { &group } else { &other };
    let rule = store.rules().await.unwrap().pop().unwrap();
    assert!(!rule.pending);
    let independent = store
        .save(None, None, config("acct_quality_b"), Utc::now(), &context())
        .await
        .unwrap();
    assert!(!apply(&store, owner, "acct_quality_b").await);
    assert_eq!(
        store
            .rules()
            .await
            .unwrap()
            .iter()
            .find(|r| r.id == independent.id)
            .unwrap()
            .revision,
        independent.revision
    );
    store
        .delete(&rule.id, rule.revision, &context())
        .await
        .unwrap();
    assert!(!apply(&store, owner, "acct_quality_a").await);
    let state = store.group_rules().await.unwrap();
    let state = state.iter().find(|g| g.id == owner.id).unwrap();
    assert_eq!((state.rule_count, state.excluded_count), (0, 1));
    let independent_a = store
        .save(None, None, config("acct_quality_a"), Utc::now(), &context())
        .await
        .unwrap();
    assert!(
        !store
            .apply_group_rule(
                owner,
                &target("acct_quality_a", Some(&independent_a)),
                Utc::now(),
                &context()
            )
            .await
            .unwrap()
    );
    assert!(
        store
            .delete_group_rule(&owner.id, owner.revision + 1, true, &context())
            .await
            .is_err()
    );
    store
        .delete_group_rule(&owner.id, owner.revision, true, &context())
        .await
        .unwrap();
    assert_eq!(store.rules().await.unwrap().len(), 2);
    db.close().await;
}

#[tokio::test]
async fn quality_group_edit_pause_resume_and_detach_preserve_execution_fences() {
    let Some(db) = TestDatabase::create("quality_group_edits").await else {
        return;
    };
    let store = setup(&db).await;
    let group = create_group(&store, QualityGroupFilter::default()).await;
    assert!(apply(&store, &group, "acct_quality_a").await);
    let rule = store.rules().await.unwrap().pop().unwrap();
    store
        .enqueue(&rule.id, rule.revision, &context())
        .await
        .unwrap();
    let claim = store.claim().await.unwrap().unwrap();
    let mut cfg = group.config.clone();
    cfg.enabled = false;
    cfg.interval_seconds = Some(17);
    let paused = store
        .save_group_rule(
            Some(&group.id),
            Some(group.revision),
            group.name.clone(),
            group.filter.clone(),
            cfg,
            &context(),
        )
        .await
        .unwrap();
    assert!(!store.current(&claim).await.unwrap());
    assert!(store.claim().await.unwrap().is_none());
    assert!(!apply(&store, &paused, "acct_quality_b").await);
    let targets = store.group_targets(&paused.id, "").await.unwrap();
    assert_eq!(targets.len(), 1);
    assert!(
        store
            .apply_group_rule(
                &paused,
                &targets[0],
                Utc::now() + Duration::seconds(17),
                &context()
            )
            .await
            .unwrap()
    );
    store
        .finish(&claim, Utc::now(), vec![answer(QualityVerdict::Incorrect)])
        .await
        .unwrap();
    let rule = store.rules().await.unwrap().pop().unwrap();
    assert_eq!(rule.config.interval_seconds, Some(17));
    assert!(!rule.config.enabled);
    assert!(!rule.pending && !rule.running);
    assert_eq!(store.runs(&rule.id).await.unwrap()[0].status, "cancelled");
    let mut cfg = paused.config.clone();
    cfg.enabled = true;
    let resumed = store
        .save_group_rule(
            Some(&paused.id),
            Some(paused.revision),
            paused.name,
            paused.filter,
            cfg,
            &context(),
        )
        .await
        .unwrap();
    let targets = store.group_targets(&resumed.id, "").await.unwrap();
    assert!(
        !store
            .apply_group_rule(&group, &targets[0], Utc::now(), &context())
            .await
            .unwrap()
    );
    assert!(
        store
            .apply_group_rule(
                &resumed,
                &targets[0],
                Utc::now() + Duration::seconds(17),
                &context()
            )
            .await
            .unwrap()
    );
    assert!(apply(&store, &resumed, "acct_quality_b").await);
    assert!(
        store
            .group_targets(&resumed.id, "")
            .await
            .unwrap()
            .is_empty()
    );
    let rules = store.rules().await.unwrap();
    store
        .delete_group_rule(&resumed.id, resumed.revision, false, &context())
        .await
        .unwrap();
    assert!(store.group_rules().await.unwrap().is_empty());
    assert_eq!(
        serde_json::to_value(store.rules().await.unwrap()).unwrap(),
        serde_json::to_value(rules).unwrap()
    );
    db.close().await;
}

#[tokio::test]
async fn quality_group_membership_account_cascade_and_owned_delete_are_safe() {
    let Some(db) = TestDatabase::create("quality_group_cascade").await else {
        return;
    };
    let store = setup(&db).await;
    let group_id = "grp_00000000000000000000000000000001";
    sqlx::query("insert into account_groups(id,name,color,enabled,created_at,updated_at) values($1,'fixture','#FFFFFFFF',true,now(),now())")
        .bind(group_id).execute(&db.pool).await.unwrap();
    sqlx::query("insert into account_group_accounts(account_group_id,provider_account_id,created_at) values($1,'acct_quality_a',now())")
        .bind(group_id).execute(&db.pool).await.unwrap();
    let group = create_group(
        &store,
        QualityGroupFilter {
            group: group_id.into(),
            statuses: vec![],
        },
    )
    .await;
    assert!(!apply(&store, &group, "acct_quality_b").await);
    assert!(apply(&store, &group, "acct_quality_a").await);
    sqlx::query("delete from account_group_accounts where account_group_id=$1")
        .bind(group_id)
        .execute(&db.pool)
        .await
        .unwrap();
    let mut config = group.config.clone();
    config.interval_seconds = Some(60);
    let edited = store
        .save_group_rule(
            Some(&group.id),
            Some(group.revision),
            group.name.clone(),
            group.filter.clone(),
            config,
            &context(),
        )
        .await
        .unwrap();
    let targets = store.group_targets(&edited.id, "").await.unwrap();
    assert_eq!(targets.len(), 1);
    assert!(
        store
            .apply_group_rule(&edited, &targets[0], Utc::now(), &context())
            .await
            .unwrap()
    );
    let claim = store.claim().await.unwrap().unwrap();
    sqlx::query("delete from provider_accounts where id='acct_quality_a'")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(!store.current(&claim).await.unwrap());
    store
        .finish(&claim, Utc::now(), vec![answer(QualityVerdict::Incorrect)])
        .await
        .unwrap();
    assert!(store.rules().await.unwrap().is_empty());
    assert!(store.detail(&claim.run_id).await.unwrap().is_none());
    assert_eq!(store.group_rules().await.unwrap()[0].excluded_count, 0);
    let global = create_group(&store, QualityGroupFilter::default()).await;
    assert!(apply(&store, &global, "acct_quality_b").await);
    store
        .delete_group_rule(&global.id, global.revision, true, &context())
        .await
        .unwrap();
    assert!(store.rules().await.unwrap().is_empty());
    db.close().await;
}
