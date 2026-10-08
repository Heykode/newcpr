use super::TestDatabase;
use chrono::{Duration, Utc};
use gateway_admin::{
    model::{MutationActor, MutationContext, quality_ops::*},
    ports::quality_ops::QualityOpsStore,
};
use gateway_store::postgres::quality_ops::PgQualityOpsStore;

mod group_safety;
mod groups;
mod native_recovery;
mod overload;
mod policy;
mod rule_templates;
mod templates;

fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::System,
        request_id: "quality-fixture".into(),
    }
}

fn config(account: &str) -> QualityRuleConfig {
    QualityRuleConfig {
        failure_template: None,
        excel_failure_threshold: 1,
        excel_recovery_threshold: None,
        detection_mode: QualityDetectionMode::Answer,
        account_id: account.into(),
        model: "fixture-model".into(),
        enabled: true,
        interval_seconds: None,
        cron: "0 */6 * * *".into(),
        timezone: "UTC".into(),
        repetitions: 1,
        prompt: "fixture question".into(),
        reference_answer: "fixture answer".into(),
        reasoning_effort: None,
        judge_group_id: "fixture-group".into(),
        judge_model: "fixture-judge".into(),
        judge_prompt: "compare".into(),
        failure_action: QualityFailureAction::None,
        failure_group_ids: Vec::new(),
        auto_restore: false,
        disable_excel_on_native_recovery: false,
    }
}

async fn setup(db: &TestDatabase) -> PgQualityOpsStore {
    for index in 0..11 {
        sqlx::query(
            "insert into provider_accounts(
            id,provider_kind,name,upstream_user_id,authentication_kind,provider_credentials_json,
            has_refresh_token,credential_observed_at,created_at,updated_at,credential_state)
            values($1,'openai',$1,$1,'oauth','{}',false,now(),now(),now(),'ready')",
        )
        .bind(format!("acct_quality_{}", char::from(b'a' + index)))
        .execute(&db.pool)
        .await
        .unwrap();
    }
    PgQualityOpsStore::new(db.pool.clone())
}

fn answer(verdict: QualityVerdict) -> QualityAnswer {
    QualityAnswer {
        probe: None,
        index: 1,
        answer: "fixture answer".into(),
        verdict,
        reason: "fixture reason".into(),
        elapsed_ms: 20,
        returned_model: Some("fixture-model".into()),
        judge_account_id: Some("acct_quality_b".into()),
    }
}

#[tokio::test]
async fn quality_initial_activation_is_due_now_but_edits_and_completion_use_the_schedule() {
    let Some(db) = TestDatabase::create("quality_first_activation").await else {
        return;
    };
    let store = setup(&db).await;
    let next = Utc::now() + Duration::seconds(120);
    let mut cfg = config("acct_quality_a");
    cfg.interval_seconds = Some(120);
    let created = store.save(None, None, cfg, next, &context()).await.unwrap();
    assert!(created.next_run_at <= Utc::now());
    let (first, duplicate) = tokio::join!(store.claim(), store.claim());
    let claims: Vec<_> = [first.unwrap(), duplicate.unwrap()]
        .into_iter()
        .flatten()
        .collect();
    assert_eq!(claims.len(), 1);
    store
        .finish(&claims[0], next, vec![answer(QualityVerdict::Correct)])
        .await
        .unwrap();
    assert!(store.claim().await.unwrap().is_none());
    let mut cfg = created.config.clone();
    cfg.prompt = "edited question".into();
    let edited = store
        .save(
            Some(&created.id),
            Some(created.revision),
            cfg,
            next,
            &context(),
        )
        .await
        .unwrap();
    assert_eq!(
        edited.next_run_at.timestamp_millis(),
        next.timestamp_millis()
    );
    assert!(store.claim().await.unwrap().is_none());
    let mut cfg = edited.config.clone();
    cfg.enabled = false;
    let paused = store
        .save(
            Some(&edited.id),
            Some(edited.revision),
            cfg,
            next,
            &context(),
        )
        .await
        .unwrap();
    assert!(store.claim().await.unwrap().is_none());
    let mut cfg = paused.config.clone();
    cfg.enabled = true;
    let enabled = store
        .save(
            Some(&paused.id),
            Some(paused.revision),
            cfg,
            next,
            &context(),
        )
        .await
        .unwrap();
    assert!(enabled.next_run_at <= Utc::now());
    let claim = store.claim().await.unwrap().unwrap();
    assert_eq!(claim.rule.id, created.id);
    assert!(store.claim().await.unwrap().is_none());

    let mut cfg = config("acct_quality_b");
    cfg.enabled = false;
    let disabled = store.save(None, None, cfg, next, &context()).await.unwrap();
    assert_eq!(
        disabled.next_run_at.timestamp_millis(),
        next.timestamp_millis()
    );
    assert!(store.claim().await.unwrap().is_none());
    let mut cfg = disabled.config.clone();
    cfg.enabled = true;
    let enabled = store
        .save(
            Some(&disabled.id),
            Some(disabled.revision),
            cfg,
            next,
            &context(),
        )
        .await
        .unwrap();
    let claim = store.claim().await.unwrap().unwrap();
    assert_eq!(claim.rule.id, enabled.id);
    db.close().await;
}

#[tokio::test]
async fn quality_claims_are_globally_bounded_and_results_are_lazy() {
    let Some(db) = TestDatabase::create("quality_claims").await else {
        return;
    };
    let store = setup(&db).await;
    for index in 0..11 {
        store
            .save(
                None,
                None,
                config(&format!("acct_quality_{}", char::from(b'a' + index))),
                Utc::now() - Duration::minutes(1),
                &context(),
            )
            .await
            .unwrap();
    }
    let claims = futures::future::join_all((0..11).map(|_| store.claim()))
        .await
        .into_iter()
        .filter_map(Result::unwrap)
        .collect::<Vec<_>>();
    assert_eq!(claims.len(), 10);
    assert_ne!(claims[0].rule.id, claims[1].rule.id);
    assert!(store.claim().await.unwrap().is_none());
    store
        .finish(
            &claims[0],
            Utc::now() + Duration::hours(6),
            vec![answer(QualityVerdict::Correct)],
        )
        .await
        .unwrap();
    let history = store.runs(&claims[0].rule.id).await.unwrap();
    assert_eq!(history[0].correct, 1);
    assert!(history[0].answers.is_empty());
    assert_eq!(
        store
            .detail(&claims[0].run_id)
            .await
            .unwrap()
            .unwrap()
            .answers
            .len(),
        1
    );
    assert!(store.claim().await.unwrap().is_some());
    db.close().await;
}

#[tokio::test]
async fn quality_pause_fences_late_completion_and_manual_run_is_deduplicated() {
    let Some(db) = TestDatabase::create("quality_pause").await else {
        return;
    };
    let store = setup(&db).await;
    let mut config = config("acct_quality_a");
    config.failure_action = QualityFailureAction::DisableScheduling;
    let original = store
        .save(None, None, config.clone(), Utc::now(), &context())
        .await
        .unwrap();
    assert!(
        store
            .save(None, None, config, Utc::now(), &context())
            .await
            .is_err()
    );
    let claim = store.claim().await.unwrap().unwrap();
    let mut changed = original.config.clone();
    changed.enabled = false;
    let updated = store
        .save(
            Some(&original.id),
            Some(original.revision),
            changed,
            Utc::now(),
            &context(),
        )
        .await
        .unwrap();
    assert!(!store.current(&claim).await.unwrap());
    store
        .finish(&claim, Utc::now(), vec![answer(QualityVerdict::Incorrect)])
        .await
        .unwrap();
    assert_eq!(
        store.detail(&claim.run_id).await.unwrap().unwrap().status,
        "cancelled"
    );
    assert!(enabled(&db).await);
    assert!(store.claim().await.unwrap().is_none());
    assert!(
        store
            .enqueue(&updated.id, original.revision, &context())
            .await
            .is_err()
    );
    store
        .enqueue(&updated.id, updated.revision, &context())
        .await
        .unwrap();
    assert!(
        store
            .enqueue(&updated.id, updated.revision, &context())
            .await
            .is_err()
    );
    let manual = store.claim().await.unwrap().unwrap();
    assert!(!manual.rule.config.enabled);
    assert!(store.current(&manual).await.unwrap());
    assert!(
        store
            .enqueue(&updated.id, updated.revision, &context())
            .await
            .is_err()
    );
    assert!(
        store
            .delete(&updated.id, original.revision, &context())
            .await
            .is_err()
    );
    store
        .delete(&updated.id, updated.revision, &context())
        .await
        .unwrap();
    assert!(!store.current(&manual).await.unwrap());
    assert!(store.detail(&manual.run_id).await.unwrap().is_none());
    db.close().await;
}

#[tokio::test]
async fn quality_paused_manual_completion_does_not_resume_schedule() {
    let Some(db) = TestDatabase::create("quality_manual_once").await else {
        return;
    };
    let store = setup(&db).await;
    let mut config = config("acct_quality_a");
    config.enabled = false;
    let rule = store
        .save(None, None, config, Utc::now(), &context())
        .await
        .unwrap();
    assert!(store.claim().await.unwrap().is_none());
    store
        .enqueue(&rule.id, rule.revision, &context())
        .await
        .unwrap();
    let queued = &store.rules().await.unwrap()[0];
    assert!(!queued.config.enabled);
    assert!(queued.pending);
    assert!(!queued.running);

    let claim = store.claim().await.unwrap().unwrap();
    assert!(!claim.rule.config.enabled);
    assert!(store.current(&claim).await.unwrap());
    let running = &store.rules().await.unwrap()[0];
    assert!(!running.config.enabled);
    assert!(!running.pending);
    assert!(running.running);
    // Even a due next occurrence cannot resume the paused schedule after completion.
    store
        .finish(&claim, Utc::now(), vec![answer(QualityVerdict::Correct)])
        .await
        .unwrap();
    let finished = &store.rules().await.unwrap()[0];
    assert!(!finished.config.enabled);
    assert!(!finished.pending);
    assert!(!finished.running);
    assert_eq!(finished.revision, rule.revision);
    assert_eq!(finished.last_status.as_deref(), Some("correct"));
    assert_eq!(
        store.detail(&claim.run_id).await.unwrap().unwrap().correct,
        1
    );
    assert!(store.claim().await.unwrap().is_none());
    // A later explicit click can request a second one-shot run.
    store
        .enqueue(&rule.id, rule.revision, &context())
        .await
        .unwrap();
    assert!(store.claim().await.unwrap().is_some());
    db.close().await;
}

#[tokio::test]
async fn quality_expired_lease_recovers_and_errors_do_not_count_as_incorrect() {
    let Some(db) = TestDatabase::create("quality_recovery").await else {
        return;
    };
    let store = setup(&db).await;
    store
        .save(None, None, config("acct_quality_a"), Utc::now(), &context())
        .await
        .unwrap();
    let old = store.claim().await.unwrap().unwrap();
    sqlx::query("update quality_rules set lease_until=now()-interval '1 second'")
        .execute(&db.pool)
        .await
        .unwrap();
    let new = store.claim().await.unwrap().unwrap();
    assert_ne!(old.lease_token, new.lease_token);
    assert!(!store.current(&old).await.unwrap());
    assert_eq!(
        store.detail(&old.run_id).await.unwrap().unwrap().status,
        "interrupted"
    );
    store
        .finish(
            &new,
            Utc::now() + Duration::hours(6),
            vec![answer(QualityVerdict::RequestError)],
        )
        .await
        .unwrap();
    let result = store.detail(&new.run_id).await.unwrap().unwrap();
    assert_eq!(result.request_errors, 1);
    assert_eq!(result.incorrect, 0);
    sqlx::query("update quality_runs set started_at=now()-interval '8 days'")
        .execute(&db.pool)
        .await
        .unwrap();
    store.cleanup().await.unwrap();
    assert!(store.runs(&new.rule.id).await.unwrap().is_empty());
    db.close().await;
}

async fn scheduled_round(
    store: &PgQualityOpsStore,
    rule: &QualityRule,
    verdicts: &[QualityVerdict],
) -> QualityRun {
    store
        .enqueue(&rule.id, rule.revision, &context())
        .await
        .unwrap();
    let claim = store.claim().await.unwrap().unwrap();
    let answers = verdicts
        .iter()
        .enumerate()
        .map(|(index, verdict)| {
            let mut result = answer(verdict.clone());
            result.index = u8::try_from(index + 1).unwrap();
            result
        })
        .collect();
    store
        .finish(&claim, Utc::now() + Duration::hours(6), answers)
        .await
        .unwrap();
    store.detail(&claim.run_id).await.unwrap().unwrap()
}

async fn enabled(db: &TestDatabase) -> bool {
    sqlx::query_scalar("select enabled from provider_accounts where id='acct_quality_a'")
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn quality_only_wrong_answers_pause_and_only_a_complete_pass_restores() {
    use QualityVerdict::{Correct, Incorrect, RequestError, Unknown};
    let Some(db) = TestDatabase::create("quality_policy_rounds").await else {
        return;
    };
    let store = setup(&db).await;
    let mut config = config("acct_quality_a");
    config.repetitions = 2;
    config.failure_action = QualityFailureAction::DisableScheduling;
    config.auto_restore = true;
    let rule = store
        .save(
            None,
            None,
            config,
            Utc::now() + Duration::hours(6),
            &context(),
        )
        .await
        .unwrap();
    let run = scheduled_round(&store, &rule, &[RequestError, Unknown]).await;
    assert!(run.action.is_none());
    assert!(enabled(&db).await);
    let run = scheduled_round(&store, &rule, &[Incorrect, RequestError]).await;
    assert_eq!(run.status, "incorrect");
    assert_eq!(run.action.as_deref(), Some("scheduling_paused"));
    assert!(!enabled(&db).await);
    let run = scheduled_round(&store, &rule, &[Correct]).await;
    assert_eq!(run.status, "unknown");
    assert!(!enabled(&db).await);
    let run = scheduled_round(&store, &rule, &[Correct, Correct]).await;
    assert_eq!(run.action.as_deref(), Some("restored"));
    assert!(enabled(&db).await);
    db.close().await;
}

#[tokio::test]
async fn quality_manual_pause_and_credential_failure_are_not_restored() {
    use QualityVerdict::{Correct, Incorrect};
    use gateway_core::account::{ProviderAccountId, ProviderAccountStore};
    use gateway_store::postgres::PgProviderAccountRepository;
    let Some(db) = TestDatabase::create("quality_manual_ownership").await else {
        return;
    };
    let store = setup(&db).await;
    let repository = PgProviderAccountRepository::new(db.pool.clone());
    let account = ProviderAccountId::new("acct_quality_a").unwrap();
    let mut config = config("acct_quality_a");
    config.failure_action = QualityFailureAction::DisableScheduling;
    config.auto_restore = true;
    let rule = store
        .save(
            None,
            None,
            config,
            Utc::now() + Duration::hours(6),
            &context(),
        )
        .await
        .unwrap();
    scheduled_round(&store, &rule, &[Incorrect]).await;
    store
        .enqueue(&rule.id, rule.revision, &context())
        .await
        .unwrap();
    let claim = store.claim().await.unwrap().unwrap();
    assert!(repository.quality_pause_is_owned(&account).await.unwrap());
    repository.set_enabled(&account, false).await.unwrap();
    assert!(!repository.quality_pause_is_owned(&account).await.unwrap());
    store
        .finish(
            &claim,
            Utc::now() + Duration::hours(6),
            vec![answer(Correct)],
        )
        .await
        .unwrap();
    assert!(!enabled(&db).await);
    assert_eq!(
        store
            .detail(&claim.run_id)
            .await
            .unwrap()
            .unwrap()
            .action
            .as_deref(),
        Some("ownership_released")
    );
    repository.set_enabled(&account, true).await.unwrap();
    scheduled_round(&store, &rule, &[Incorrect]).await;
    sqlx::query(
        "update provider_accounts set credential_state='expired' where id='acct_quality_a'",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let run = scheduled_round(&store, &rule, &[Correct]).await;
    assert_eq!(run.action.as_deref(), Some("restore_blocked"));
    assert!(!enabled(&db).await);
    db.close().await;
}

#[tokio::test]
async fn quality_does_not_own_a_preexisting_pause_or_restore_when_disabled() {
    use QualityVerdict::{Correct, Incorrect};
    let Some(db) = TestDatabase::create("quality_preexisting_pause").await else {
        return;
    };
    let store = setup(&db).await;
    sqlx::query("update provider_accounts set enabled=false where id='acct_quality_a'")
        .execute(&db.pool)
        .await
        .unwrap();
    let mut config = config("acct_quality_a");
    config.failure_action = QualityFailureAction::DisableScheduling;
    config.auto_restore = true;
    let rule = store
        .save(
            None,
            None,
            config,
            Utc::now() + Duration::hours(6),
            &context(),
        )
        .await
        .unwrap();
    assert_eq!(
        scheduled_round(&store, &rule, &[Incorrect])
            .await
            .action
            .as_deref(),
        Some("no_change")
    );
    assert!(
        scheduled_round(&store, &rule, &[Correct])
            .await
            .action
            .is_none()
    );
    assert!(!enabled(&db).await);
    sqlx::query("update provider_accounts set enabled=true where id='acct_quality_a'")
        .execute(&db.pool)
        .await
        .unwrap();
    scheduled_round(&store, &rule, &[Incorrect]).await;
    let mut config = rule.config.clone();
    config.auto_restore = false;
    let rule = store
        .save(
            Some(&rule.id),
            Some(rule.revision),
            config,
            Utc::now() + Duration::hours(6),
            &context(),
        )
        .await
        .unwrap();
    assert!(
        scheduled_round(&store, &rule, &[Correct])
            .await
            .action
            .is_none()
    );
    assert!(!enabled(&db).await);
    store
        .delete(&rule.id, rule.revision, &context())
        .await
        .unwrap();
    assert!(!enabled(&db).await);
    db.close().await;
}

#[tokio::test]
async fn quality_group_actions_preserve_other_memberships_and_block_deleted_groups() {
    use QualityVerdict::{Correct, Incorrect};
    let Some(db) = TestDatabase::create("quality_group_restore").await else {
        return;
    };
    let store = setup(&db).await;
    let remove = "grp_00000000000000000000000000000001";
    let keep = "grp_00000000000000000000000000000002";
    let absent = "grp_00000000000000000000000000000003";
    for id in [remove, keep, absent] {
        sqlx::query(
            "insert into account_groups(id,name,color,enabled,created_at,updated_at)
            values($1,$1,'#FFFFFFFF',true,now(),now())",
        )
        .bind(id)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    for id in [remove, keep] {
        sqlx::query("insert into account_group_accounts values($1,'acct_quality_a',now())")
            .bind(id)
            .execute(&db.pool)
            .await
            .unwrap();
    }
    let mut config = config("acct_quality_a");
    config.failure_action = QualityFailureAction::RemoveGroups;
    config.failure_group_ids = vec![remove.into(), absent.into()];
    config.auto_restore = true;
    let rule = store
        .save(
            None,
            None,
            config,
            Utc::now() + Duration::hours(6),
            &context(),
        )
        .await
        .unwrap();
    assert_eq!(
        scheduled_round(&store, &rule, &[Incorrect])
            .await
            .action
            .as_deref(),
        Some("groups_removed")
    );
    let groups: Vec<String> = sqlx::query_scalar(
        "select account_group_id from account_group_accounts where provider_account_id='acct_quality_a'",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(groups, [keep]);
    assert!(enabled(&db).await);
    sqlx::query("update account_group_accounts set created_at=created_at+interval '1 microsecond' where provider_account_id='acct_quality_a'")
        .execute(&db.pool).await.unwrap();
    assert_eq!(
        scheduled_round(&store, &rule, &[Correct])
            .await
            .action
            .as_deref(),
        Some("restored")
    );
    let count: i64 = sqlx::query_scalar(
        "select count(*) from account_group_accounts where provider_account_id='acct_quality_a'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(count, 2);
    scheduled_round(&store, &rule, &[Incorrect]).await;
    sqlx::query("insert into account_group_accounts values($1,'acct_quality_a',now())")
        .bind(absent)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        scheduled_round(&store, &rule, &[Correct])
            .await
            .action
            .as_deref(),
        Some("restore_blocked")
    );
    sqlx::query("delete from account_group_accounts where provider_account_id='acct_quality_a' and account_group_id=$1")
        .bind(absent).execute(&db.pool).await.unwrap();
    sqlx::query("delete from account_groups where id=$1")
        .bind(remove)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        scheduled_round(&store, &rule, &[Correct])
            .await
            .action
            .as_deref(),
        Some("restore_blocked")
    );
    db.close().await;
}

#[tokio::test]
async fn quality_replaced_identity_cannot_be_paused_by_an_old_answer() {
    let Some(db) = TestDatabase::create("quality_identity_fence").await else {
        return;
    };
    let store = setup(&db).await;
    let mut config = config("acct_quality_a");
    config.failure_action = QualityFailureAction::DisableScheduling;
    store
        .save(None, None, config, Utc::now(), &context())
        .await
        .unwrap();
    let claim = store.claim().await.unwrap().unwrap();
    sqlx::query("update provider_accounts set upstream_user_id='replacement-user' where id='acct_quality_a'")
        .execute(&db.pool).await.unwrap();
    store
        .finish(
            &claim,
            Utc::now() + Duration::hours(6),
            vec![answer(QualityVerdict::Incorrect)],
        )
        .await
        .unwrap();
    assert!(enabled(&db).await);
    assert_eq!(
        store
            .detail(&claim.run_id)
            .await
            .unwrap()
            .unwrap()
            .action
            .as_deref(),
        Some("identity_changed")
    );
    db.close().await;
}

#[tokio::test]
async fn quality_recovery_never_undoes_an_independent_excel_403_pause() {
    use QualityVerdict::{Correct, Incorrect};
    use gateway_core::account::{ProviderAccountId, ProviderAccountStore};
    use gateway_store::postgres::PgProviderAccountRepository;
    let Some(db) = TestDatabase::create("quality_excel403").await else {
        return;
    };
    let store = setup(&db).await;
    sqlx::query("update provider_accounts set responses_upstream='excel',excel_auto_disable_on_403=true, excel_403_action='pause_account' where id='acct_quality_a'")
        .execute(&db.pool).await.unwrap();
    let mut config = config("acct_quality_a");
    config.failure_action = QualityFailureAction::DisableScheduling;
    config.auto_restore = true;
    let rule = store
        .save(
            None,
            None,
            config,
            Utc::now() + Duration::hours(6),
            &context(),
        )
        .await
        .unwrap();
    scheduled_round(&store, &rule, &[Incorrect]).await;
    let repository = PgProviderAccountRepository::new(db.pool.clone());
    let account = repository
        .get_account(&ProviderAccountId::new("acct_quality_a").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(repository.apply_excel_403_action(&account).await.unwrap());
    assert!(!repository.apply_excel_403_action(&account).await.unwrap());
    let run = scheduled_round(&store, &rule, &[Correct]).await;
    assert_eq!(run.action.as_deref(), Some("ownership_released"));
    assert!(!enabled(&db).await);
    db.close().await;
}

#[tokio::test]
async fn quality_account_rename_keeps_recovery_ownership() {
    use QualityVerdict::{Correct, Incorrect};
    use gateway_admin::{model::accounts::UpdateAccount, ports::store::AccountStore};
    use gateway_core::account::AccountWeight;
    let Some(db) = TestDatabase::create("quality_rename").await else {
        return;
    };
    let store = setup(&db).await;
    let mut config = config("acct_quality_a");
    config.failure_action = QualityFailureAction::DisableScheduling;
    config.auto_restore = true;
    let rule = store
        .save(
            None,
            None,
            config,
            Utc::now() + Duration::hours(6),
            &context(),
        )
        .await
        .unwrap();
    scheduled_round(&store, &rule, &[Incorrect]).await;
    super::admin_account_store(&db.pool)
        .update_account(
            UpdateAccount {
                excel_recovery: None,
                purchase_cost: None,
                egress_mode: None,
                account_id: "acct_quality_a".into(),
                custom_name: Some(Some("renamed account".into())),
                enabled: false,
                concurrency_limit: None,
                weight: AccountWeight::DEFAULT,
                group_ids: Vec::new(),
                outbound_proxy: None,
                turn_state_injection_enabled: None,
                responses_upstream: None,
                excel_models: None,
                excel_models_follow_global: None,
                excel_cache_creation_as_input: None,
                excel_ignore_encrypted_content: None,
                request_proxy_source: None,
                excel_auto_disable_on_403: None,
                excel_403_action: Default::default(),
                model_access: None,
            },
            &context(),
        )
        .await
        .unwrap();
    assert_eq!(
        scheduled_round(&store, &rule, &[Correct])
            .await
            .action
            .as_deref(),
        Some("restored")
    );
    assert!(enabled(&db).await);
    db.close().await;
}
