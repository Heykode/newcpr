use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gateway_admin::{
    model::{
        MutationContext,
        account_groups::{
            AccountGroupColor, AccountGroupListQuery, AccountGroupMemberFact, AccountGroupMutation,
            AccountGroupPage, AccountGroupRef, DeleteAccountGroup, NewAccountGroup,
            SetAccountGroupEnabled, UpdateAccountGroup,
        },
        group_monitor::{GroupMonitorFacts, GroupMonitorItem, GroupMonitorReport},
        notifications::{
            AlertConditionKind, AlertObservation, BarkChannelView, BarkLevel,
            ClaimedNotificationDelivery, GroupAlertPolicy, NotificationChannelKind,
            NotificationChannelsView, ReplaceNotificationChannels, SmtpChannelView, SmtpSecurity,
            StoredBarkChannel, StoredNotificationChannels, StoredSmtpChannel,
            TestNotificationCommand,
        },
        settings::{AdminApiKey, AdminApiKeyMutation, ReplaceRuntimeSettings, RuntimeSettings},
    },
    ports::store::{AccountGroupStore, AdminStoreResult, SettingsStore},
};
use gateway_core::routing::AccountGroupId;
use secrecy::{ExposeSecret as _, SecretString};

struct NotificationStore {
    policy: Mutex<GroupAlertPolicy>,
    policy_after_test_enqueue: Option<GroupAlertPolicy>,
    channels: StoredNotificationChannels,
    observations: Mutex<Vec<AlertObservation>>,
    saved: Mutex<Option<ReplaceNotificationChannels>>,
    finished: Mutex<Vec<(bool, Option<String>)>>,
    claimed: Mutex<Option<ClaimedNotificationDelivery>>,
    cancelled: Mutex<Vec<String>>,
}

#[async_trait]
impl AccountGroupStore for NotificationStore {
    async fn claim_notification_delivery(
        &self,
        _: DateTime<Utc>,
    ) -> AdminStoreResult<Option<ClaimedNotificationDelivery>> {
        Ok(self.claimed.lock().unwrap().take())
    }

    async fn cancel_notification_delivery(
        &self,
        id: &str,
        _: DateTime<Utc>,
    ) -> AdminStoreResult<()> {
        self.cancelled.lock().unwrap().push(id.to_owned());
        Ok(())
    }
    async fn enqueue_test_notification(
        &self,
        delivery: ClaimedNotificationDelivery,
        _: DateTime<Utc>,
    ) -> AdminStoreResult<String> {
        if let Some(policy) = &self.policy_after_test_enqueue {
            *self.policy.lock().unwrap() = policy.clone();
        }
        Ok(delivery.id)
    }

    async fn finish_notification_delivery(
        &self,
        _: &str,
        sent: bool,
        error: Option<&str>,
        _: DateTime<Utc>,
    ) -> AdminStoreResult<()> {
        self.finished
            .lock()
            .unwrap()
            .push((sent, error.map(str::to_owned)));
        Ok(())
    }

    async fn load_group_alert_policy(
        &self,
        group_id: &AccountGroupId,
    ) -> AdminStoreResult<GroupAlertPolicy> {
        let policy = self.policy.lock().unwrap();
        assert_eq!(group_id, &policy.group_id);
        Ok(policy.clone())
    }

    async fn apply_group_alert_observations(
        &self,
        observations: &[AlertObservation],
        _: DateTime<Utc>,
    ) -> AdminStoreResult<()> {
        self.observations
            .lock()
            .unwrap()
            .extend_from_slice(observations);
        Ok(())
    }

    async fn load_group_monitor(&self, _: DateTime<Utc>) -> AdminStoreResult<GroupMonitorFacts> {
        Err(super::unavailable("unused monitor read"))
    }

    async fn save_group_monitor(&self, _: &GroupMonitorReport, _: u64) -> AdminStoreResult<()> {
        Err(super::unavailable("unused monitor write"))
    }

    async fn read_group_monitor(
        &self,
        _: &[AccountGroupId],
    ) -> AdminStoreResult<Option<GroupMonitorReport>> {
        Err(super::unavailable("unused monitor snapshot"))
    }

    async fn list_account_groups(
        &self,
        _: AccountGroupListQuery,
    ) -> AdminStoreResult<AccountGroupPage> {
        Err(super::unavailable("unused groups"))
    }

    async fn load_account_group_members(
        &self,
        _: &[AccountGroupId],
    ) -> AdminStoreResult<Vec<AccountGroupMemberFact>> {
        Err(super::unavailable("unused group members"))
    }

    async fn create_account_group(
        &self,
        _: NewAccountGroup,
        _: &MutationContext,
    ) -> AdminStoreResult<AccountGroupMutation> {
        Err(super::unavailable("unused group create"))
    }

    async fn update_account_group(
        &self,
        _: UpdateAccountGroup,
        _: &MutationContext,
    ) -> AdminStoreResult<AccountGroupMutation> {
        Err(super::unavailable("unused group update"))
    }

    async fn set_account_group_enabled(
        &self,
        _: SetAccountGroupEnabled,
        _: &MutationContext,
    ) -> AdminStoreResult<AccountGroupMutation> {
        Err(super::unavailable("unused group enable"))
    }

    async fn delete_account_group(
        &self,
        _: DeleteAccountGroup,
        _: &MutationContext,
    ) -> AdminStoreResult<AccountGroupMutation> {
        Err(super::unavailable("unused group delete"))
    }
}

#[async_trait]
impl SettingsStore for NotificationStore {
    async fn replace_notification_channels(
        &self,
        command: ReplaceNotificationChannels,
        _: &MutationContext,
    ) -> AdminStoreResult<NotificationChannelsView> {
        let view = NotificationChannelsView {
            smtp: command.smtp.view.clone(),
            bark: command.bark.view.clone(),
            last_test: None,
            updated_at: Utc::now(),
        };
        *self.saved.lock().unwrap() = Some(command);
        Ok(view)
    }

    async fn load_notification_channels(&self) -> AdminStoreResult<StoredNotificationChannels> {
        Ok(self.channels.clone())
    }

    async fn load_runtime_settings(&self) -> AdminStoreResult<RuntimeSettings> {
        Err(super::unavailable("unused settings"))
    }

    async fn admin_api_key_exists(&self) -> AdminStoreResult<bool> {
        Err(super::unavailable("unused admin key"))
    }

    async fn replace_runtime_settings(
        &self,
        _: ReplaceRuntimeSettings,
        _: &MutationContext,
    ) -> AdminStoreResult<RuntimeSettings> {
        Err(super::unavailable("unused settings"))
    }

    async fn replace_admin_api_key(
        &self,
        _: AdminApiKey,
        _: &MutationContext,
    ) -> AdminStoreResult<AdminApiKeyMutation> {
        Err(super::unavailable("unused admin key"))
    }

    async fn delete_admin_api_key(
        &self,
        _: &MutationContext,
    ) -> AdminStoreResult<AdminApiKeyMutation> {
        Err(super::unavailable("unused admin key"))
    }
}

async fn observe(
    item: GroupMonitorItem,
    policy: GroupAlertPolicy,
    channels: StoredNotificationChannels,
) -> Vec<AlertObservation> {
    let store = Arc::new(NotificationStore {
        policy: Mutex::new(policy),
        policy_after_test_enqueue: None,
        channels,
        observations: Mutex::new(Vec::new()),
        saved: Mutex::new(None),
        finished: Mutex::new(Vec::new()),
        claimed: Mutex::new(None),
        cancelled: Mutex::new(Vec::new()),
    });
    let services = super::AdminHarness::new()
        .settings(store.clone())
        .account_groups(store.clone())
        .build()
        .await;
    services
        .notifications()
        .observe(&GroupMonitorReport {
            generated_at: Utc::now(),
            refreshing: false,
            pending_group_ids: Vec::new(),
            items: vec![item],
        })
        .await
        .unwrap();
    store.observations.lock().unwrap().clone()
}

fn item() -> GroupMonitorItem {
    GroupMonitorItem {
        group: AccountGroupRef {
            id: AccountGroupId::new("grp_88888888888888888888888888888888").unwrap(),
            name: "Synthetic".to_owned(),
            color: AccountGroupColor::parse("#2563EBFF").unwrap(),
            enabled: true,
        },
        total_accounts: 3,
        eligible_accounts: 2,
        estimated_accounts: 2,
        used_slots: Some(9),
        total_slots: 10,
        remaining_usd: Some(20.0),
        remaining_status: "ready",
        expected_expiry_usd: Some(999.0),
        expiry_status: "ready",
        consume_usd_per_minute: Some(1.0),
        quota_consume_usd_per_minute: Some(2.0),
        eta_minutes: Some(8.0),
        eta_status: "ready",
        low_sample: false,
        earliest_reset_at: None,
    }
}

fn channels(enabled: bool) -> StoredNotificationChannels {
    StoredNotificationChannels {
        smtp: StoredSmtpChannel {
            view: SmtpChannelView {
                enabled,
                host: "smtp.example.com".to_owned(),
                port: 587,
                security: SmtpSecurity::StartTls,
                username: None,
                password_set: false,
                from_name: None,
                from_email: Some("alerts@example.com".to_owned()),
            },
            password: None,
        },
        bark: StoredBarkChannel {
            view: BarkChannelView {
                enabled,
                server_url: "https://push.example.com".to_owned(),
                device_key_set: enabled,
                level: BarkLevel::Active,
                sound: None,
                volume: 5,
                call: false,
            },
            device_key: None,
        },
        updated_at: Utc::now(),
    }
}

#[tokio::test]
async fn group_thresholds_activate_only_with_a_configured_route() {
    let mut policy = GroupAlertPolicy::defaults(item().group.id, Utc::now());
    policy.enabled = true;
    policy.email_enabled = true;
    policy.email_recipients = vec!["ops@example.com".to_owned()];
    policy.bark_enabled = true;
    let active = observe(item(), policy.clone(), channels(true)).await;
    assert_eq!(active.len(), 4);
    assert!(
        active
            .iter()
            .find(|row| row.kind == AlertConditionKind::Concurrency)
            .unwrap()
            .active
    );
    assert!(
        active
            .iter()
            .find(|row| row.kind == AlertConditionKind::Eta)
            .unwrap()
            .active
    );
    assert!(
        !active
            .iter()
            .find(|row| row.kind == AlertConditionKind::QuotaZero)
            .unwrap()
            .active
    );
    assert!(
        !active
            .iter()
            .find(|row| row.kind == AlertConditionKind::Availability)
            .unwrap()
            .active
    );
    assert!(active.iter().all(|row| !row.summary.contains("999.0")));

    let unrouted = observe(item(), policy, channels(false)).await;
    assert!(unrouted.iter().all(|row| !row.active));
}

#[tokio::test]
async fn unknown_values_never_trigger_and_zero_conditions_do() {
    let mut policy = GroupAlertPolicy::defaults(item().group.id, Utc::now());
    policy.enabled = true;
    policy.bark_enabled = true;
    let mut snapshot = item();
    snapshot.used_slots = None;
    snapshot.eta_minutes = None;
    snapshot.eta_status = "unknown";
    snapshot.remaining_usd = Some(0.0);
    snapshot.remaining_status = "empty";
    snapshot.eligible_accounts = 0;
    let observations = observe(snapshot.clone(), policy.clone(), channels(true)).await;
    for kind in [AlertConditionKind::Concurrency, AlertConditionKind::Eta] {
        let unknown = observations.iter().find(|row| row.kind == kind).unwrap();
        assert!(unknown.enabled);
        assert!(!unknown.active);
        assert!(unknown.current_value.is_none());
    }
    assert!(
        observations
            .iter()
            .find(|row| row.kind == AlertConditionKind::QuotaZero)
            .unwrap()
            .active
    );
    assert!(
        observations
            .iter()
            .find(|row| row.kind == AlertConditionKind::Availability)
            .unwrap()
            .active
    );
    snapshot.total_accounts = 0;
    assert!(
        observe(snapshot, policy, channels(true))
            .await
            .iter()
            .find(|row| row.kind == AlertConditionKind::Availability)
            .unwrap()
            .active
    );
}

#[tokio::test]
async fn display_only_snapshots_never_trigger_or_recover_alerts() {
    let mut policy = GroupAlertPolicy::defaults(item().group.id, Utc::now());
    policy.enabled = true;
    policy.bark_enabled = true;
    let store = Arc::new(NotificationStore {
        policy: Mutex::new(policy),
        policy_after_test_enqueue: None,
        channels: channels(true),
        observations: Mutex::new(Vec::new()),
        saved: Mutex::new(None),
        finished: Mutex::new(Vec::new()),
        claimed: Mutex::new(None),
        cancelled: Mutex::new(Vec::new()),
    });
    let services = super::AdminHarness::new()
        .settings(store.clone())
        .account_groups(store.clone())
        .build()
        .await;
    for (refreshing, pending_group_ids) in [(true, vec![]), (false, vec![item().group.id])] {
        services
            .notifications()
            .observe(&GroupMonitorReport {
                generated_at: Utc::now(),
                refreshing,
                pending_group_ids,
                items: vec![item()],
            })
            .await
            .unwrap();
    }
    assert!(store.observations.lock().unwrap().is_empty());
}

#[tokio::test]
async fn notification_save_normalizes_bark_only_and_persists_safe_delivery_failure() {
    let mut configured = channels(true);
    configured.smtp.password = Some(SecretString::from(" synthetic password "));
    configured.smtp.view.password_set = true;
    configured.bark.device_key = Some(SecretString::from("synthetic-existing"));
    let store = Arc::new(NotificationStore {
        policy: Mutex::new(GroupAlertPolicy::defaults(item().group.id, Utc::now())),
        policy_after_test_enqueue: None,
        channels: configured.clone(),
        observations: Mutex::new(Vec::new()),
        saved: Mutex::new(None),
        finished: Mutex::new(Vec::new()),
        claimed: Mutex::new(None),
        cancelled: Mutex::new(Vec::new()),
    });
    let services = super::AdminHarness::new()
        .settings(store.clone())
        .account_groups(store.clone())
        .build()
        .await;
    let mut command = ReplaceNotificationChannels {
        smtp: configured.smtp,
        bark: configured.bark,
    };
    command.bark.device_key = Some(SecretString::from(" synthetic-device/ \n"));
    let context = MutationContext {
        actor: gateway_admin::model::MutationActor::System,
        request_id: "notification-test".to_owned(),
    };
    services
        .notifications()
        .replace_channels(command.clone(), &context)
        .await
        .unwrap();
    {
        let saved = store.saved.lock().unwrap();
        let saved = saved.as_ref().unwrap();
        assert_eq!(
            saved.bark.device_key.as_ref().unwrap().expose_secret(),
            "synthetic-device"
        );
        assert_eq!(
            saved.smtp.password.as_ref().unwrap().expose_secret(),
            " synthetic password "
        );
    }
    command.smtp.password = None;
    command.smtp.view.password_set = false;
    command.bark.device_key = None;
    command.bark.view.device_key_set = false;
    services
        .notifications()
        .replace_channels(command, &context)
        .await
        .unwrap();
    {
        let saved = store.saved.lock().unwrap();
        let saved = saved.as_ref().unwrap();
        assert!(saved.bark.device_key.is_none());
        assert!(saved.bark.view.device_key_set);
        assert!(saved.smtp.password.is_none());
        assert!(saved.smtp.view.password_set);
    }
    let error = services
        .notifications()
        .test(TestNotificationCommand {
            channel: NotificationChannelKind::Email,
            target: "ops@example.com".to_owned(),
            group_id: None,
        })
        .await
        .unwrap_err();
    assert_eq!(error.message(), "通知传输未启用");
    assert_eq!(
        *store.finished.lock().unwrap(),
        vec![(false, Some(error.message().to_owned()))]
    );
}

fn night_rule() -> gateway_admin::model::notifications::BarkScheduleRule {
    gateway_admin::model::notifications::BarkScheduleRule {
        id: "night".to_owned(),
        name: "Night".to_owned(),
        enabled: true,
        start_time: "23:30".to_owned(),
        end_time: "09:00".to_owned(),
        level: Some(BarkLevel::Critical),
        sound: Some("alarm".to_owned()),
        volume: Some(8),
        call: Some(true),
    }
}

#[test]
fn bark_schedules_match_timezone_midnight_boundaries_and_list_priority() {
    let mut policy = GroupAlertPolicy::defaults(item().group.id, Utc::now());
    let channel = channels(true).bark.view;
    let at = |time: &str| time.parse::<DateTime<Utc>>().unwrap();
    assert_eq!(
        policy
            .bark_rule_at(&channel, at("2026-11-15T15:29:00Z"))
            .unwrap()
            .id,
        "default"
    );
    policy.bark_schedule.rules.push(night_rule());
    policy.bark_schedule.default_enabled = false;
    policy.bark_schedule.normalize().unwrap();
    assert!(
        policy
            .bark_rule_at(&channel, at("2026-11-15T15:29:00Z"))
            .is_none()
    );
    for time in [
        "2026-11-15T15:30:00Z",
        "2026-11-15T16:00:00Z",
        "2026-11-16T00:59:59Z",
    ] {
        let rule = policy.bark_rule_at(&channel, at(time)).unwrap();
        assert_eq!(rule.id, "night");
        assert_eq!(
            (rule.level, rule.volume, rule.call),
            (BarkLevel::Critical, 8, true)
        );
    }
    assert!(
        policy
            .bark_rule_at(&channel, at("2026-11-16T01:00:00Z"))
            .is_none()
    );
    let mut overlapping = night_rule();
    overlapping.id = "overlap".to_owned();
    overlapping.level = Some(BarkLevel::Active);
    policy.bark_schedule.rules.insert(0, overlapping);
    assert_eq!(
        policy
            .bark_rule_at(&channel, at("2026-11-15T16:00:00Z"))
            .unwrap()
            .id,
        "overlap"
    );
    policy.bark_schedule.rules[0].enabled = false;
    assert_eq!(
        policy
            .bark_rule_at(&channel, at("2026-11-15T16:00:00Z"))
            .unwrap()
            .id,
        "night"
    );
    policy.bark_schedule.timezone = "America/New_York".to_owned();
    // Both occurrences of a repeated DST clock minute select the same presentation.
    let first = policy
        .bark_rule_at(&channel, at("2026-11-01T05:30:00Z"))
        .unwrap();
    let second = policy
        .bark_rule_at(&channel, at("2026-11-01T06:30:00Z"))
        .unwrap();
    assert_eq!(first.fingerprint(), second.fingerprint());
}

#[test]
fn bark_schedule_validation_rejects_ambiguous_and_unbounded_inputs() {
    let mut schedule = gateway_admin::model::notifications::BarkSchedule::default();
    schedule.rules.push(night_rule());
    for value in ["9:00", "24:00", "09:60", "invalid"] {
        let mut invalid = schedule.clone();
        invalid.rules[0].start_time = value.to_owned();
        assert!(invalid.normalize().is_err());
    }
    let mut invalid = schedule.clone();
    invalid.rules[0].end_time = invalid.rules[0].start_time.clone();
    assert!(invalid.normalize().is_err());
    let mut invalid = schedule.clone();
    invalid.rules.push(night_rule());
    assert!(invalid.normalize().is_err());
    let mut invalid = schedule.clone();
    invalid.rules[0].volume = Some(11);
    assert!(invalid.normalize().is_err());
    let mut invalid = schedule.clone();
    invalid.timezone = "invalid/timezone".to_owned();
    assert!(invalid.normalize().is_err());
    let mut invalid = schedule.clone();
    invalid.rules = (0..25)
        .map(|index| {
            let mut rule = night_rule();
            rule.id = format!("rule-{index}");
            rule
        })
        .collect();
    assert!(invalid.normalize().is_err());
    let mut policy = GroupAlertPolicy::defaults(item().group.id, Utc::now());
    policy.bark_level = Some(BarkLevel::Critical);
    policy.bark_volume = Some(7);
    let channel = channels(true).bark.view;
    let rule = policy.bark_rule_at(&channel, Utc::now()).unwrap();
    assert_eq!((rule.level, rule.volume), (BarkLevel::Critical, 7));
    assert!(
        BarkLevel::Critical.delivery_strength(0, false)
            > BarkLevel::TimeSensitive.delivery_strength(10, true)
    );
}

#[tokio::test]
async fn quiet_bark_period_keeps_monitor_confirmation_and_email_routes() {
    let mut policy = GroupAlertPolicy::defaults(item().group.id, Utc::now());
    policy.enabled = true;
    policy.bark_enabled = true;
    policy.email_enabled = true;
    policy.email_recipients = vec!["ops@example.com".to_owned()];
    policy.bark_schedule.default_enabled = false;
    let rows = observe(item(), policy, channels(true)).await;
    let concurrency = rows
        .iter()
        .find(|row| row.kind == AlertConditionKind::Concurrency)
        .unwrap();
    assert!(concurrency.active && concurrency.enabled);
    assert!(!concurrency.bark_enabled);
    assert_eq!(concurrency.confirmation_seconds, 20);
    assert_eq!(concurrency.email_recipients, ["ops@example.com"]);
}

#[tokio::test]
async fn dispatch_cancels_out_of_period_reminder_and_quiet_test_never_sends() {
    let mut policy = GroupAlertPolicy::defaults(item().group.id, Utc::now());
    policy.enabled = true;
    policy.bark_enabled = true;
    policy.bark_schedule.default_enabled = false;
    let store = Arc::new(NotificationStore {
        claimed: Mutex::new(Some(ClaimedNotificationDelivery {
            id: "queued-before-quiet".to_owned(),
            group_id: Some(policy.group_id.clone()),
            condition: Some(AlertConditionKind::Concurrency),
            channel: NotificationChannelKind::Bark,
            target: "default".to_owned(),
            subject: "Synthetic".to_owned(),
            body: "Synthetic".to_owned(),
            bark_level: BarkLevel::Active,
            bark_sound: None,
            bark_volume: 5,
            bark_call: false,
            bark_rule_key: None,
            incident_activated_at: Some(Utc::now()),
            test: false,
        })),
        policy: Mutex::new(policy),
        policy_after_test_enqueue: None,
        channels: channels(true),
        observations: Mutex::new(Vec::new()),
        saved: Mutex::new(None),
        finished: Mutex::new(Vec::new()),
        cancelled: Mutex::new(Vec::new()),
    });
    let services = super::AdminHarness::new()
        .settings(store.clone())
        .account_groups(store.clone())
        .build()
        .await;
    assert!(services.notifications().dispatch_one().await.unwrap());
    assert_eq!(*store.cancelled.lock().unwrap(), ["queued-before-quiet"]);
    assert!(
        store.finished.lock().unwrap().is_empty(),
        "cancelled is not delivered or a transport failure"
    );
    let group_id = store.policy.lock().unwrap().group_id.clone();
    let error = services
        .notifications()
        .test(TestNotificationCommand {
            channel: NotificationChannelKind::Bark,
            target: "default".to_owned(),
            group_id: Some(group_id),
        })
        .await
        .unwrap_err();
    assert!(error.message().contains("免打扰"));
    assert!(store.finished.lock().unwrap().is_empty());
}

#[tokio::test]
async fn group_bark_test_rechecks_rule_after_enqueue_and_reports_cancellation() {
    let policy = GroupAlertPolicy::defaults(item().group.id, Utc::now());
    let group_id = policy.group_id.clone();
    let mut quiet = policy.clone();
    quiet.bark_schedule.default_enabled = false;
    let store = Arc::new(NotificationStore {
        policy: Mutex::new(policy),
        policy_after_test_enqueue: Some(quiet),
        channels: channels(true),
        observations: Mutex::new(Vec::new()),
        saved: Mutex::new(None),
        finished: Mutex::new(Vec::new()),
        claimed: Mutex::new(None),
        cancelled: Mutex::new(Vec::new()),
    });
    let services = super::AdminHarness::new()
        .settings(store.clone())
        .account_groups(store.clone())
        .build()
        .await;
    let error = services
        .notifications()
        .test(TestNotificationCommand {
            channel: NotificationChannelKind::Bark,
            target: "default".to_owned(),
            group_id: Some(group_id),
        })
        .await
        .unwrap_err();
    assert!(error.message().contains("规则已变化"));
    assert_eq!(store.cancelled.lock().unwrap().len(), 1);
    assert!(
        store.finished.lock().unwrap().is_empty(),
        "a cancelled test must not be reported as sent"
    );
}
