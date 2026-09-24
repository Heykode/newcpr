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
    policy: GroupAlertPolicy,
    channels: StoredNotificationChannels,
    observations: Mutex<Vec<AlertObservation>>,
    saved: Mutex<Option<ReplaceNotificationChannels>>,
    finished: Mutex<Vec<(bool, Option<String>)>>,
}

#[async_trait]
impl AccountGroupStore for NotificationStore {
    async fn enqueue_test_notification(
        &self,
        delivery: ClaimedNotificationDelivery,
        _: DateTime<Utc>,
    ) -> AdminStoreResult<String> {
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
        assert_eq!(group_id, &self.policy.group_id);
        Ok(self.policy.clone())
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
        policy,
        channels,
        observations: Mutex::new(Vec::new()),
        saved: Mutex::new(None),
        finished: Mutex::new(Vec::new()),
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
    assert!(
        !observations
            .iter()
            .any(|row| row.kind == AlertConditionKind::Concurrency)
    );
    assert!(
        !observations
            .iter()
            .any(|row| row.kind == AlertConditionKind::Eta)
    );
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
        policy,
        channels: channels(true),
        observations: Mutex::new(Vec::new()),
        saved: Mutex::new(None),
        finished: Mutex::new(Vec::new()),
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
        policy: GroupAlertPolicy::defaults(item().group.id, Utc::now()),
        channels: configured.clone(),
        observations: Mutex::new(Vec::new()),
        saved: Mutex::new(None),
        finished: Mutex::new(Vec::new()),
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
