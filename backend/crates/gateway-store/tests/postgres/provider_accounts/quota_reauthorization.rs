//! Reauthorization preserves quota facts only for the same upstream identity.
use super::*;

#[tokio::test]
async fn reauthorization_preserves_disabled_quota_warmup_and_window_hint() {
    let Some(database) = TestDatabase::create("reauthorization_quota_window").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    for (slot, replacement) in [
        ("acct_same_quota", None),
        (
            "acct_new_quota",
            Some(ProviderAccountIdentity::new(
                "replacement-user".into(),
                Some("replacement-workspace".into()),
            )),
        ),
    ] {
        let id = ProviderAccountId::new(slot).unwrap();
        let mut seed = account(slot, slot);
        seed.enabled = false;
        repository.insert_provider_account(seed).await.unwrap();
        let observed_at = SystemTime::from(Utc::now());
        let document = json!({"rate_limit":{"allowed":false,"secondary_window":{"used_percent":42,"limit_window_seconds":604800}}});
        let observation = QuotaObservation {
            account_id: id.clone(),
            expected_revision: CredentialRevision::new(1).unwrap(),
            quota: OpaqueProviderData::new(document.as_object().unwrap().clone()),
            observed_at,
            state: QuotaState::exhausted(QuotaEvidence::UsageLimitReached, observed_at, None)
                .with_limit_window_minutes(Some(10080)),
            quota_pause: None,
            plan_type: None,
        };
        assert_eq!(
            repository
                .compare_and_swap_quota(observation.clone())
                .await
                .unwrap(),
            QuotaWriteOutcome::Updated
        );
        sqlx::query("update provider_accounts set quota_warmup_state = '{\"claimed\":true}', quota_paused = true where id = $1")
            .bind(slot).execute(&database.pool).await.unwrap();
        let same_identity = replacement.is_none();
        repository
            .rotate_with_workspace_policy(
                RotateProviderAccount {
                    relogin_operation_id: None,
                    scope: ProviderAccountAdminScope {
                        provider_kind: "openai".into(),
                    },
                    profile: profile(slot, "updated remote profile"),
                    replacement_identity: replacement,
                    credential: credential_update(slot, 1, "synthetic-reauthorized"),
                    audit: audit(&format!("reauthorize-{slot}"), "reauthorize", slot),
                },
                false,
                true,
            )
            .await
            .unwrap();
        let current = repository.get_account(&id).await.unwrap().unwrap();
        assert!(!current.enabled());
        assert_eq!(current.revision().get(), 2);
        assert_eq!(current.plan_type(), Some("team"));
        assert_eq!(
            repository
                .compare_and_swap_quota(observation)
                .await
                .unwrap(),
            QuotaWriteOutcome::Conflict
        );
        let snapshots = repository
            .get_quotas(std::slice::from_ref(&id))
            .await
            .unwrap();
        let (warmup, paused): (serde_json::Value, bool) = sqlx::query_as(
            "select quota_warmup_state, quota_paused from provider_accounts where id = $1",
        )
        .bind(slot)
        .fetch_one(&database.pool)
        .await
        .unwrap();
        if same_identity {
            assert_eq!(snapshots.len(), 1);
            assert_eq!(snapshots[0].expected_revision, current.revision());
            assert_eq!(
                snapshots[0].quota.expose_to_provider(),
                document.as_object().unwrap()
            );
            assert_eq!(snapshots[0].state, current.quota());
            assert_eq!(current.quota().limit_window_minutes(), Some(10080));
            assert_eq!(warmup, json!({"claimed":true}));
            assert!(paused);
        } else {
            assert!(snapshots.is_empty());
            assert_eq!(current.quota(), QuotaState::unknown());
            assert_eq!(warmup, json!({}));
            assert!(!paused);
        }
    }
    database.close().await;
}

#[tokio::test]
async fn newer_quota_window_hint_survives_older_snapshot_and_clears_on_success() {
    let Some(database) = TestDatabase::create("quota_window_cas").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    let id = ProviderAccountId::new("acct_window_cas").unwrap();
    repository
        .insert_provider_account(account(id.as_str(), "window-owner"))
        .await
        .unwrap();
    let revision = CredentialRevision::new(1).unwrap();
    let now = SystemTime::now();
    repository
        .apply_quota_access(QuotaAccessChange {
            account_id: id.clone(),
            expected_revision: revision,
            state: QuotaState::exhausted(
                QuotaEvidence::UsageLimitReached,
                now + Duration::from_secs(1),
                None,
            )
            .with_limit_window_minutes(Some(10080)),
        })
        .await
        .unwrap();
    repository
        .compare_and_swap_quota(QuotaObservation {
            account_id: id.clone(),
            expected_revision: revision,
            quota: OpaqueProviderData::new(
                json!({"rate_limit":{"allowed":true}})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            observed_at: now,
            state: QuotaState::allowed(now),
            quota_pause: None,
            plan_type: None,
        })
        .await
        .unwrap();
    let current = repository.get_account(&id).await.unwrap().unwrap();
    assert_eq!(current.quota().limit_window_minutes(), Some(10080));
    assert!(current.quota().is_exhausted());
    assert_eq!(
        repository
            .get_quotas(std::slice::from_ref(&id))
            .await
            .unwrap()[0]
            .state,
        current.quota()
    );
    repository
        .apply_quota_access(QuotaAccessChange {
            account_id: id.clone(),
            expected_revision: revision,
            state: QuotaState::allowed(now + Duration::from_secs(2)),
        })
        .await
        .unwrap();
    assert_eq!(
        repository
            .get_account(&id)
            .await
            .unwrap()
            .unwrap()
            .quota()
            .limit_window_minutes(),
        None
    );
    database.close().await;
}
