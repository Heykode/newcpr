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

fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::System,
        request_id: "notification-test".to_owned(),
    }
}

async fn group(pool: &sqlx::PgPool) -> AccountGroupId {
    let id = AccountGroupId::new("grp_99999999999999999999999999999999").unwrap();
    PgAccountGroupRepository::new(pool.clone())
        .create_account_group(
            NewAccountGroup {
                id: id.clone(),
                name: "Synthetic Alerts".to_owned(),
                description: None,
                color: AccountGroupColor::parse("#2563EBFF").unwrap(),
                disable_fast: false,
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
        summary: "synthetic alert".to_owned(),
    };
    repository
        .apply_observations(std::slice::from_ref(&observation), base)
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
    repository
        .apply_observations(
            std::slice::from_ref(&recovered),
            base + Duration::seconds(90),
        )
        .await
        .unwrap();
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
    repository
        .apply_observations(&[observation.clone()], base + Duration::seconds(10))
        .await
        .unwrap();
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
        .apply_observations(&[observation], base + Duration::seconds(40))
        .await
        .unwrap();
    assert_eq!(
        repository.recent_deliveries(None, 10).await.unwrap().len(),
        1
    );
    db.close().await;
}
