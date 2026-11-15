use chrono::{Duration, Utc};
use gateway_admin::{
    model::{
        MutationActor, MutationContext,
        account_groups::{AccountGroupColor, NewAccountGroup},
        notifications::{
            AlertConditionKind, AlertObservation, BarkChannelView, BarkLevel,
            ClaimedNotificationDelivery, GroupAlertPolicy, NotificationChannelKind,
            ReplaceNotificationChannels, SmtpChannelView, SmtpSecurity, StoredBarkChannel,
            StoredSmtpChannel,
        },
    },
    ports::store::AccountGroupStore,
};
use gateway_core::routing::AccountGroupId;
use gateway_store::postgres::{PgAccountGroupRepository, PgNotificationRepository};
use secrecy::SecretString;

use super::TestDatabase;

fn episode_observation(group_id: AccountGroupId, kind: AlertConditionKind) -> AlertObservation {
    AlertObservation {
        group_id,
        group_name: "Synthetic episode".to_owned(),
        kind,
        enabled: true,
        active: true,
        current_value: Some(1.0),
        threshold: 20.0,
        confirmation_seconds: 0,
        email_recipients: vec!["ops@example.com".to_owned()],
        bark_enabled: true,
        bark_level: BarkLevel::Active,
        bark_sound: None,
        bark_volume: 5,
        bark_call: false,
        bark_rule_key: None,
        summary: "synthetic episode".to_owned(),
    }
}

#[tokio::test]
async fn alternating_conditions_share_an_episode_until_all_recover() {
    let Some(db) = TestDatabase::create("notification_episode").await else {
        return;
    };
    let group_id = group(&db.pool).await;
    let repository = PgNotificationRepository::new(db.pool.clone());
    let base = Utc::now();
    let mut eta = episode_observation(group_id.clone(), AlertConditionKind::Eta);
    let mut concurrency = episode_observation(group_id.clone(), AlertConditionKind::Concurrency);
    concurrency.active = false;
    repository
        .apply_observations(&[eta.clone(), concurrency.clone()], base)
        .await
        .unwrap();
    let delivered = repository.claim_delivery(base).await.unwrap().unwrap();
    repository
        .finish_delivery(&delivered.id, true, None, base)
        .await
        .unwrap();
    let retry = repository.claim_delivery(base).await.unwrap().unwrap();
    repository
        .finish_delivery(&retry.id, false, Some("synthetic failure"), base)
        .await
        .unwrap();
    eta.active = false;
    concurrency.active = true;
    for second in [10, 20, 30, 40, 50, 60] {
        repository
            .apply_observations(
                &[eta.clone(), concurrency.clone()],
                base + Duration::seconds(second),
            )
            .await
            .unwrap();
    }
    assert_eq!(
        repository.recent_deliveries(None, 100).await.unwrap().len(),
        2
    );
    let claimed = repository
        .claim_delivery(base + Duration::seconds(60))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        claimed.id, retry.id,
        "the original failed target can retry across condition changes"
    );
    repository
        .finish_delivery(&retry.id, true, None, base + Duration::seconds(60))
        .await
        .unwrap();
    eta.active = true;
    concurrency.active = false;
    for second in [70, 80, 90, 100] {
        repository
            .apply_observations(
                &[eta.clone(), concurrency.clone()],
                base + Duration::seconds(second),
            )
            .await
            .unwrap();
    }
    assert_eq!(
        repository.recent_deliveries(None, 100).await.unwrap().len(),
        2
    );
    eta.active = false;
    for second in [110, 120, 130, 140] {
        repository
            .apply_observations(
                &[eta.clone(), concurrency.clone()],
                base + Duration::seconds(second),
            )
            .await
            .unwrap();
    }
    assert!(repository.active_alerts().await.unwrap().is_empty());
    eta.active = true;
    repository
        .apply_observations(&[eta, concurrency], base + Duration::seconds(150))
        .await
        .unwrap();
    assert_eq!(
        repository.recent_deliveries(None, 100).await.unwrap().len(),
        4
    );
    db.close().await;
}

#[tokio::test]
async fn unknown_samples_and_missing_ticks_interrupt_recovery_without_rearming() {
    let Some(db) = TestDatabase::create("notification_unknown_recovery").await else {
        return;
    };
    let repository = PgNotificationRepository::new(db.pool.clone());
    let base = Utc::now();
    let mut observation = episode_observation(group(&db.pool).await, AlertConditionKind::Eta);
    repository
        .apply_observations(&[observation.clone()], base)
        .await
        .unwrap();
    observation.active = false;
    for second in [10, 20] {
        repository
            .apply_observations(&[observation.clone()], base + Duration::seconds(second))
            .await
            .unwrap();
    }
    observation.current_value = None;
    repository
        .apply_observations(&[observation.clone()], base + Duration::seconds(30))
        .await
        .unwrap();
    observation.current_value = Some(30.0);
    repository
        .apply_observations(&[observation.clone()], base + Duration::seconds(40))
        .await
        .unwrap();
    assert_eq!(repository.active_alerts().await.unwrap().len(), 1);
    // The absent 50-second sample must restart recovery at 60, not finish it at 70.
    for second in [60, 70, 80] {
        repository
            .apply_observations(&[observation.clone()], base + Duration::seconds(second))
            .await
            .unwrap();
        assert_eq!(repository.active_alerts().await.unwrap().len(), 1);
    }
    observation.active = true;
    repository
        .apply_observations(&[observation.clone()], base + Duration::seconds(90))
        .await
        .unwrap();
    assert_eq!(
        repository.recent_deliveries(None, 100).await.unwrap().len(),
        2
    );
    observation.active = false;
    // A disabled condition with no telemetry does not prevent legitimate recovery.
    let mut disabled = observation.clone();
    disabled.kind = AlertConditionKind::Concurrency;
    disabled.enabled = false;
    disabled.current_value = None;
    for second in [100, 110, 120, 130] {
        repository
            .apply_observations(
                &[observation.clone(), disabled.clone()],
                base + Duration::seconds(second),
            )
            .await
            .unwrap();
    }
    assert!(repository.active_alerts().await.unwrap().is_empty());
    db.close().await;
}

#[tokio::test]
async fn concurrent_conditions_deduplicate_destinations_and_keep_groups_independent() {
    let Some(db) = TestDatabase::create("notification_group_lock").await else {
        return;
    };
    let group_id = group(&db.pool).await;
    let repository = PgNotificationRepository::new(db.pool.clone());
    let base = Utc::now();
    let eta = [episode_observation(
        group_id.clone(),
        AlertConditionKind::Eta,
    )];
    let concurrency = [episode_observation(
        group_id,
        AlertConditionKind::Concurrency,
    )];
    let (a, b) = tokio::join!(
        repository.apply_observations(&eta, base),
        repository.apply_observations(&concurrency, base)
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(
        repository.recent_deliveries(None, 100).await.unwrap().len(),
        2
    );
    let second_group = group(&db.pool).await;
    let other = episode_observation(second_group, AlertConditionKind::Eta);
    repository.apply_observations(&[other], base).await.unwrap();
    assert_eq!(
        repository.recent_deliveries(None, 100).await.unwrap().len(),
        4
    );
    db.close().await;
}

#[tokio::test]
async fn unknown_observation_restarts_pending_confirmation() {
    let Some(db) = TestDatabase::create("notification_unknown_confirmation").await else {
        return;
    };
    let repository = PgNotificationRepository::new(db.pool.clone());
    let base = Utc::now();
    let mut observation = episode_observation(group(&db.pool).await, AlertConditionKind::Eta);
    observation.confirmation_seconds = 20;
    repository
        .apply_observations(&[observation.clone()], base)
        .await
        .unwrap();
    observation.current_value = None;
    observation.active = false;
    repository
        .apply_observations(&[observation.clone()], base + Duration::seconds(10))
        .await
        .unwrap();
    observation.current_value = Some(1.0);
    observation.active = true;
    for second in [20, 30] {
        repository
            .apply_observations(&[observation.clone()], base + Duration::seconds(second))
            .await
            .unwrap();
        assert!(
            repository
                .recent_deliveries(None, 100)
                .await
                .unwrap()
                .is_empty()
        );
    }
    repository
        .apply_observations(&[observation], base + Duration::seconds(40))
        .await
        .unwrap();
    assert_eq!(
        repository.recent_deliveries(None, 100).await.unwrap().len(),
        2
    );
    db.close().await;
}

fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::System,
        request_id: "notification-test".to_owned(),
    }
}

async fn group(pool: &sqlx::PgPool) -> AccountGroupId {
    let id = AccountGroupId::new(format!("grp_{}", uuid::Uuid::now_v7().simple())).unwrap();
    PgAccountGroupRepository::new(pool.clone())
        .create_account_group(
            NewAccountGroup {
                allow_image_generation: true,
                id: id.clone(),
                name: format!("Synthetic {}", id.as_str()),
                description: None,
                color: AccountGroupColor::parse("#2563EBFF").unwrap(),
                disable_fast: false,
                fast_mode: Default::default(),
            },
            &context(),
        )
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn channels_are_write_only_and_group_policy_round_trips() {
    let Some(db) = TestDatabase::create("notification_channels").await else {
        return;
    };
    let group_id = group(&db.pool).await;
    let repository = PgNotificationRepository::new(db.pool.clone());
    let initial = repository.load_channels().await.unwrap();
    assert!(!initial.smtp.view.password_set);
    assert!(!initial.bark.view.device_key_set);
    let view = repository
        .replace_channels(
            ReplaceNotificationChannels {
                smtp: StoredSmtpChannel {
                    view: SmtpChannelView {
                        enabled: true,
                        host: "smtp.example.com".to_owned(),
                        port: 587,
                        security: SmtpSecurity::StartTls,
                        username: Some("synthetic-user".to_owned()),
                        password_set: true,
                        from_name: Some("CPR".to_owned()),
                        from_email: Some("alerts@example.com".to_owned()),
                    },
                    password: Some(SecretString::from("synthetic-password")),
                },
                bark: StoredBarkChannel {
                    view: BarkChannelView {
                        enabled: true,
                        server_url: "https://push.example.com".to_owned(),
                        device_key_set: true,
                        level: BarkLevel::Critical,
                        sound: Some("alarm".to_owned()),
                        volume: 8,
                        call: true,
                    },
                    device_key: Some(SecretString::from("synthetic-device-key")),
                },
            },
            &context(),
        )
        .await
        .unwrap();
    assert!(view.smtp.password_set);
    assert!(view.bark.device_key_set);
    let stored = repository.load_channels().await.unwrap();
    let debug = format!("{stored:?}");
    assert!(!debug.contains("synthetic-password"));
    assert!(!debug.contains("synthetic-device-key"));

    let mut policy = GroupAlertPolicy::defaults(group_id.clone(), Utc::now());
    policy.enabled = true;
    policy.email_enabled = true;
    policy.email_recipients = vec!["ops@example.com".to_owned()];
    policy.bark_enabled = true;
    let saved = repository
        .replace_policy(policy.clone(), &context())
        .await
        .unwrap();
    assert_eq!(saved.group_id, group_id);
    assert_eq!(repository.load_policy(&group_id).await.unwrap(), saved);
    policy.bark_schedule.default_enabled = false;
    policy
        .bark_schedule
        .rules
        .push(gateway_admin::model::notifications::BarkScheduleRule {
            id: "night".to_owned(),
            name: "Night".to_owned(),
            enabled: true,
            start_time: "23:30".to_owned(),
            end_time: "09:00".to_owned(),
            level: Some(BarkLevel::Critical),
            sound: None,
            volume: Some(8),
            call: Some(true),
        });
    repository
        .replace_policy(policy.clone(), &context())
        .await
        .unwrap();
    assert_eq!(
        repository
            .load_policy(&group_id)
            .await
            .unwrap()
            .bark_schedule,
        policy.bark_schedule
    );
    let other_group = group(&db.pool).await;
    assert!(
        repository
            .load_policy(&other_group)
            .await
            .unwrap()
            .bark_schedule
            .rules
            .is_empty()
    );
    db.close().await;
}

#[tokio::test]
async fn bark_escalation_is_once_per_incident_and_keeps_original_activation() {
    let Some(db) = TestDatabase::create("bark_escalation").await else {
        return;
    };
    let repository = PgNotificationRepository::new(db.pool.clone());
    let mut observation =
        episode_observation(group(&db.pool).await, AlertConditionKind::Concurrency);
    let base = Utc::now();
    repository
        .apply_observations(&[observation.clone()], base)
        .await
        .unwrap();
    while let Some(delivery) = repository.claim_delivery(base).await.unwrap() {
        repository
            .finish_delivery(&delivery.id, true, None, base)
            .await
            .unwrap();
    }
    observation.bark_level = BarkLevel::Critical;
    observation.bark_volume = 8;
    observation.bark_call = true;
    let rows = [observation.clone()];
    let (one, two) = tokio::join!(
        repository.apply_observations(&rows, base + Duration::seconds(10)),
        repository.apply_observations(&rows, base + Duration::seconds(10))
    );
    one.unwrap();
    two.unwrap();
    let escalated = repository
        .claim_delivery(base + Duration::seconds(10))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(escalated.channel, NotificationChannelKind::Bark);
    assert_eq!(escalated.bark_level, BarkLevel::Critical);
    assert_eq!(
        escalated.incident_activated_at.unwrap().timestamp_micros(),
        base.timestamp_micros()
    );
    repository
        .finish_delivery(&escalated.id, true, None, base + Duration::seconds(10))
        .await
        .unwrap();
    assert_eq!(
        repository.recent_deliveries(None, 100).await.unwrap().len(),
        3
    );
    observation.bark_sound = Some("different-sound".to_owned());
    repository
        .apply_observations(&[observation.clone()], base + Duration::seconds(20))
        .await
        .unwrap();
    observation.bark_level = BarkLevel::Active;
    repository
        .apply_observations(&[observation.clone()], base + Duration::seconds(30))
        .await
        .unwrap();
    assert!(
        repository
            .claim_delivery(base + Duration::seconds(30))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        repository.recent_deliveries(None, 100).await.unwrap().len(),
        3
    );
    observation.active = false;
    for second in [40, 50, 60, 70] {
        repository
            .apply_observations(&[observation.clone()], base + Duration::seconds(second))
            .await
            .unwrap();
    }
    observation.active = true;
    repository
        .apply_observations(&[observation], base + Duration::seconds(80))
        .await
        .unwrap();
    assert_eq!(
        repository.recent_deliveries(None, 100).await.unwrap().len(),
        5
    );
    db.close().await;
}

#[tokio::test]
async fn quiet_period_then_window_sends_without_reconfirming_or_repeating_email() {
    let Some(db) = TestDatabase::create("bark_quiet_window").await else {
        return;
    };
    let repository = PgNotificationRepository::new(db.pool.clone());
    let mut observation = episode_observation(group(&db.pool).await, AlertConditionKind::Eta);
    let base = Utc::now();
    observation.bark_enabled = false;
    repository
        .apply_observations(&[observation.clone()], base)
        .await
        .unwrap();
    let email = repository.claim_delivery(base).await.unwrap().unwrap();
    assert_eq!(email.channel, NotificationChannelKind::Email);
    repository
        .finish_delivery(&email.id, true, None, base)
        .await
        .unwrap();
    observation.bark_enabled = true;
    repository
        .apply_observations(&[observation.clone()], base + Duration::seconds(10))
        .await
        .unwrap();
    let bark = repository
        .claim_delivery(base + Duration::seconds(10))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bark.channel, NotificationChannelKind::Bark);
    assert_eq!(
        bark.incident_activated_at.unwrap().timestamp_micros(),
        base.timestamp_micros()
    );
    // The sender rechecked the clock after claim and found that the window ended.
    repository
        .cancel_delivery(&bark.id, base + Duration::seconds(11))
        .await
        .unwrap();
    observation.bark_enabled = false;
    repository
        .apply_observations(&[observation.clone()], base + Duration::seconds(20))
        .await
        .unwrap();
    assert!(
        repository
            .claim_delivery(base + Duration::seconds(20))
            .await
            .unwrap()
            .is_none()
    );
    observation.bark_enabled = true;
    repository
        .apply_observations(&[observation.clone()], base + Duration::seconds(30))
        .await
        .unwrap();
    let resumed = repository
        .claim_delivery(base + Duration::seconds(30))
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        resumed.id, bark.id,
        "an unsent, cancelled reminder can resume next window"
    );
    repository
        .finish_delivery(&resumed.id, true, None, base + Duration::seconds(30))
        .await
        .unwrap();
    repository
        .apply_observations(&[observation], base + Duration::seconds(40))
        .await
        .unwrap();
    let deliveries = repository.recent_deliveries(None, 100).await.unwrap();
    assert_eq!(
        deliveries
            .iter()
            .filter(|row| row.channel == "email")
            .count(),
        1
    );
    assert_eq!(
        deliveries.iter().filter(|row| row.status == "sent").count(),
        2
    );
    assert_eq!(
        deliveries
            .iter()
            .filter(|row| row.status == "cancelled")
            .count(),
        1
    );
    db.close().await;
}

#[tokio::test]
async fn bark_retry_is_bounded_and_stronger_rule_supersedes_unsent_reminder() {
    let Some(db) = TestDatabase::create("bark_retry_rules").await else {
        return;
    };
    let repository = PgNotificationRepository::new(db.pool.clone());
    let mut observation = episode_observation(group(&db.pool).await, AlertConditionKind::QuotaZero);
    observation.email_recipients.clear();
    let base = Utc::now();
    repository
        .apply_observations(&[observation.clone()], base)
        .await
        .unwrap();
    let first = repository.claim_delivery(base).await.unwrap().unwrap();
    repository
        .finish_delivery(&first.id, false, Some("synthetic failure"), base)
        .await
        .unwrap();
    observation.bark_level = BarkLevel::Critical;
    repository
        .apply_observations(&[observation.clone()], base + Duration::seconds(10))
        .await
        .unwrap();
    let stronger = repository
        .claim_delivery(base + Duration::seconds(10))
        .await
        .unwrap()
        .unwrap();
    assert_ne!(first.id, stronger.id);
    assert_eq!(stronger.bark_level, BarkLevel::Critical);
    let records = repository.recent_deliveries(None, 10).await.unwrap();
    assert_eq!(
        records
            .iter()
            .find(|row| row.id == first.id)
            .unwrap()
            .status,
        "cancelled"
    );
    repository
        .finish_delivery(
            &stronger.id,
            false,
            Some("synthetic failure"),
            base + Duration::seconds(10),
        )
        .await
        .unwrap();
    for retry in 1..5 {
        let now = base + Duration::seconds(10 + retry * 60);
        let claimed = repository.claim_delivery(now).await.unwrap().unwrap();
        assert_eq!(claimed.id, stronger.id);
        repository
            .finish_delivery(&claimed.id, false, Some("synthetic failure"), now)
            .await
            .unwrap();
    }
    repository
        .apply_observations(&[observation], base + Duration::seconds(260))
        .await
        .unwrap();
    assert!(
        repository
            .claim_delivery(base + Duration::seconds(600))
            .await
            .unwrap()
            .is_none()
    );
    let records = repository.recent_deliveries(None, 10).await.unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(
        records
            .iter()
            .find(|row| row.id == stronger.id)
            .unwrap()
            .attempts,
        5
    );
    db.close().await;
}

#[tokio::test]
async fn bark_schedule_upgrade_preserves_legacy_policy_and_pending_delivery() {
    let old = sqlx::migrate::Migrator {
        migrations: super::TEST_MIGRATOR
            .iter()
            .filter(|migration| migration.version < 95)
            .cloned()
            .collect::<Vec<_>>()
            .into(),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    let Some(db) = TestDatabase::create_with_migrator("bark_schedule_upgrade", &old).await else {
        return;
    };
    let id = group(&db.pool).await;
    let now = Utc::now();
    sqlx::query("insert into account_group_alert_policies(group_id,enabled,concurrency_threshold,bark_enabled,updated_at)
                 values($1,true,88,true,$2)")
        .bind(id.as_str()).bind(now).execute(&db.pool).await.unwrap();
    sqlx::query("insert into account_group_alert_incidents(group_id,condition_kind,first_active_at,activated_at,updated_at)
                 values($1,'concurrency',$2,$2,$2)")
        .bind(id.as_str()).bind(now).execute(&db.pool).await.unwrap();
    sqlx::query("insert into notification_outbox(id,group_id,condition_kind,channel,target,subject,body,next_attempt_at,created_at)
                 values('legacy-bark',$1,'concurrency','bark','default','Synthetic','Synthetic',$2,$2)")
        .bind(id.as_str()).bind(now).execute(&db.pool).await.unwrap();
    super::TEST_MIGRATOR.run(&db.pool).await.unwrap();
    let repository = PgNotificationRepository::new(db.pool.clone());
    let policy = repository.load_policy(&id).await.unwrap();
    assert!(policy.enabled && policy.bark_enabled && policy.bark_schedule.default_enabled);
    assert_eq!(policy.concurrency.threshold, 88.0);
    assert!(policy.bark_schedule.rules.is_empty());
    let legacy = repository.claim_delivery(now).await.unwrap().unwrap();
    assert_eq!(legacy.id, "legacy-bark");
    assert_eq!(
        legacy.incident_activated_at.unwrap().timestamp_micros(),
        now.timestamp_micros()
    );
    assert!(legacy.bark_rule_key.is_none());
    repository
        .finish_delivery(&legacy.id, true, None, now)
        .await
        .unwrap();
    let mut observation = episode_observation(id, AlertConditionKind::Concurrency);
    observation.email_recipients.clear();
    repository
        .apply_observations(&[observation], now + Duration::seconds(10))
        .await
        .unwrap();
    assert_eq!(
        repository.recent_deliveries(None, 100).await.unwrap().len(),
        1,
        "legacy sent reminder still deduplicates"
    );
    db.close().await;
}

#[tokio::test]
async fn alert_incident_confirms_once_recovers_and_retries_targets_independently() {
    let Some(db) = TestDatabase::create("notification_incident").await else {
        return;
    };
    let group_id = group(&db.pool).await;
    let repository = PgNotificationRepository::new(db.pool.clone());
    let base = Utc::now();
    let observation = AlertObservation {
        group_id: group_id.clone(),
        group_name: "Synthetic Alerts".to_owned(),
        kind: AlertConditionKind::Concurrency,
        enabled: true,
        active: true,
        current_value: Some(95.0),
        threshold: 90.0,
        confirmation_seconds: 20,
        email_recipients: vec!["ops@example.com".to_owned()],
        bark_enabled: true,
        bark_level: BarkLevel::Critical,
        bark_sound: Some("alarm".to_owned()),
        bark_volume: 8,
        bark_call: true,
        bark_rule_key: None,
        summary: "synthetic alert".to_owned(),
    };
    repository
        .apply_observations(std::slice::from_ref(&observation), base)
        .await
        .unwrap();
    repository
        .apply_observations(
            std::slice::from_ref(&observation),
            base + Duration::seconds(10),
        )
        .await
        .unwrap();
    repository
        .apply_observations(
            std::slice::from_ref(&observation),
            base + Duration::seconds(19),
        )
        .await
        .unwrap();
    assert!(
        repository
            .recent_deliveries(Some(&group_id), 10)
            .await
            .unwrap()
            .is_empty()
    );
    repository
        .apply_observations(
            std::slice::from_ref(&observation),
            base + Duration::seconds(20),
        )
        .await
        .unwrap();
    assert_eq!(
        repository
            .recent_deliveries(Some(&group_id), 10)
            .await
            .unwrap()
            .len(),
        2
    );
    repository
        .apply_observations(
            std::slice::from_ref(&observation),
            base + Duration::seconds(40),
        )
        .await
        .unwrap();
    assert_eq!(
        repository
            .recent_deliveries(Some(&group_id), 10)
            .await
            .unwrap()
            .len(),
        2
    );

    let first = repository
        .claim_delivery(base + Duration::seconds(20))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.condition, Some(AlertConditionKind::Concurrency));
    repository
        .finish_delivery(&first.id, true, None, base + Duration::seconds(21))
        .await
        .unwrap();
    let second = repository
        .claim_delivery(base + Duration::seconds(20))
        .await
        .unwrap()
        .unwrap();
    repository
        .finish_delivery(
            &second.id,
            false,
            Some("synthetic failure"),
            base + Duration::seconds(21),
        )
        .await
        .unwrap();
    assert!(
        repository
            .claim_delivery(base + Duration::seconds(30))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .claim_delivery(base + Duration::seconds(82))
            .await
            .unwrap()
            .is_some()
    );

    let mut recovered = observation.clone();
    recovered.active = false;
    for second in [90, 100, 110] {
        repository
            .apply_observations(
                std::slice::from_ref(&recovered),
                base + Duration::seconds(second),
            )
            .await
            .unwrap();
    }
    repository
        .apply_observations(
            std::slice::from_ref(&recovered),
            base + Duration::seconds(120),
        )
        .await
        .unwrap();
    repository
        .apply_observations(
            std::slice::from_ref(&observation),
            base + Duration::seconds(121),
        )
        .await
        .unwrap();
    repository
        .apply_observations(
            std::slice::from_ref(&observation),
            base + Duration::seconds(131),
        )
        .await
        .unwrap();
    repository
        .apply_observations(
            std::slice::from_ref(&observation),
            base + Duration::seconds(141),
        )
        .await
        .unwrap();
    assert_eq!(
        repository
            .recent_deliveries(Some(&group_id), 20)
            .await
            .unwrap()
            .len(),
        4
    );
    db.close().await;
}

#[tokio::test]
async fn test_delivery_is_claimed_directly_and_failure_does_not_retry() {
    let Some(db) = TestDatabase::create("notification_test_delivery").await else {
        return;
    };
    let repository = PgNotificationRepository::new(db.pool.clone());
    let now = Utc::now();
    let id = repository
        .enqueue_test(
            ClaimedNotificationDelivery {
                id: "test-delivery".to_owned(),
                group_id: None,
                condition: None,
                channel: NotificationChannelKind::Bark,
                target: "default".to_owned(),
                subject: "test".to_owned(),
                body: "test".to_owned(),
                bark_level: BarkLevel::Active,
                bark_sound: None,
                bark_volume: 5,
                bark_call: false,
                bark_rule_key: None,
                incident_activated_at: None,
                test: true,
            },
            now,
        )
        .await
        .unwrap();
    repository
        .finish_delivery(&id, false, Some("synthetic failure"), now)
        .await
        .unwrap();
    let records = repository.recent_deliveries(None, 10).await.unwrap();
    assert_eq!(records[0].status, "failed");
    assert_eq!(records[0].attempts, 1);
    assert_eq!(repository.latest_test().await.unwrap().unwrap().id, id);
    assert!(
        repository
            .claim_delivery(now + Duration::minutes(5))
            .await
            .unwrap()
            .is_none()
    );
    db.close().await;
}

#[tokio::test]
async fn concurrent_first_observations_enqueue_only_once_and_recovery_cancels_pending() {
    let Some(db) = TestDatabase::create("notification_concurrent").await else {
        return;
    };
    let group_id = group(&db.pool).await;
    let repository = PgNotificationRepository::new(db.pool.clone());
    let base = Utc::now();
    let mut observation = AlertObservation {
        group_id: group_id.clone(),
        group_name: "Synthetic".to_owned(),
        kind: AlertConditionKind::QuotaZero,
        enabled: true,
        active: true,
        current_value: Some(0.0),
        threshold: 0.0,
        confirmation_seconds: 0,
        email_recipients: vec!["ops@example.com".to_owned()],
        bark_enabled: true,
        bark_level: BarkLevel::Active,
        bark_sound: None,
        bark_volume: 5,
        bark_call: false,
        bark_rule_key: None,
        summary: "synthetic zero quota".to_owned(),
    };
    let rows = [observation.clone()];
    let (first, second) = tokio::join!(
        repository.apply_observations(&rows, base),
        repository.apply_observations(&rows, base),
    );
    first.unwrap();
    second.unwrap();
    assert_eq!(
        repository
            .recent_deliveries(Some(&group_id), 10)
            .await
            .unwrap()
            .len(),
        2
    );
    observation.active = false;
    for second in [10, 20, 30] {
        repository
            .apply_observations(
                std::slice::from_ref(&observation),
                base + Duration::seconds(second),
            )
            .await
            .unwrap();
    }
    repository
        .apply_observations(&[observation.clone()], base + Duration::seconds(40))
        .await
        .unwrap();
    // An older snapshot must not rearm an already recovered condition.
    observation.active = true;
    repository
        .apply_observations(&[observation], base + Duration::seconds(5))
        .await
        .unwrap();
    assert!(
        repository
            .claim_delivery(base + Duration::seconds(41))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .recent_deliveries(Some(&group_id), 10)
            .await
            .unwrap()
            .iter()
            .all(|row| row.status == "failed")
    );
    db.close().await;
}

#[tokio::test]
async fn pending_confirmation_requires_continuous_abnormal_samples() {
    let Some(db) = TestDatabase::create("notification_confirmation").await else {
        return;
    };
    let group_id = group(&db.pool).await;
    let repository = PgNotificationRepository::new(db.pool.clone());
    let base = Utc::now();
    let mut observation = AlertObservation {
        group_id,
        group_name: "Synthetic".to_owned(),
        kind: AlertConditionKind::Concurrency,
        enabled: true,
        active: true,
        current_value: Some(95.0),
        threshold: 90.0,
        confirmation_seconds: 20,
        email_recipients: Vec::new(),
        bark_enabled: true,
        bark_level: BarkLevel::Active,
        bark_sound: None,
        bark_volume: 5,
        bark_call: false,
        bark_rule_key: None,
        summary: "synthetic concurrency".to_owned(),
    };
    repository
        .apply_observations(&[observation.clone()], base)
        .await
        .unwrap();
    observation.active = false;
    repository
        .apply_observations(&[observation.clone()], base + Duration::seconds(10))
        .await
        .unwrap();
    observation.active = true;
    repository
        .apply_observations(&[observation.clone()], base + Duration::seconds(20))
        .await
        .unwrap();
    assert!(
        repository
            .recent_deliveries(None, 10)
            .await
            .unwrap()
            .is_empty()
    );
    repository
        .apply_observations(&[observation.clone()], base + Duration::seconds(30))
        .await
        .unwrap();
    repository
        .apply_observations(&[observation], base + Duration::seconds(40))
        .await
        .unwrap();
    assert_eq!(
        repository.recent_deliveries(None, 10).await.unwrap().len(),
        1
    );
    db.close().await;
}
