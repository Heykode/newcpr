use async_trait::async_trait;
use gateway_core::policy::ClientApiKeyId;

use gateway_admin::{
    model::{
        AdminErrorKind, MutationContext, Revision,
        client_keys::{
            ClientKeyCursor, ClientKeyCursorValue, ClientKeyListQuery, ClientKeyPage,
            ClientKeyPageSize, ClientKeyRecord, ClientKeySecret, ClientKeySort, ClientKeySortField,
            DeleteClientKey, NewClientKey, SetClientKeyEnabled, SortDirection, UpdateClientKey,
        },
    },
    ports::store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult, ClientKeyStore},
};

struct UnusedClientKeyStore;

#[async_trait]
impl ClientKeyStore for UnusedClientKeyStore {
    async fn budget_reference_accounts(
        &self,
        _: &gateway_core::policy::ClientApiKeyId,
        _: &str,
        _: &str,
    ) -> AdminStoreResult<gateway_admin::model::client_keys::BudgetReferenceAccountPage> {
        Err(unused())
    }
    async fn client_key_budget_window(
        &self,
        _: &gateway_core::policy::ClientApiKeyId,
        _: gateway_admin::model::client_keys::ClientKeyBudgetWindowPeriod,
    ) -> AdminStoreResult<gateway_admin::model::client_keys::ClientKeyBudgetWindow> {
        Err(unused())
    }
    async fn set_client_key_budget_window(
        &self,
        _: gateway_admin::model::client_keys::SetClientKeyBudgetWindow,
        _: &gateway_admin::model::MutationContext,
    ) -> AdminStoreResult<u64> {
        Err(unused())
    }
    async fn get_client_key(
        &self,
        _: &gateway_core::policy::ClientApiKeyId,
    ) -> AdminStoreResult<Option<gateway_admin::model::client_keys::ClientKeyRecord>> {
        Err(unused())
    }
    async fn client_key_budget(
        &self,
        _: &gateway_core::policy::ClientApiKeyId,
    ) -> AdminStoreResult<Option<gateway_core::engine::budget::ClientBudgetStatus>> {
        Err(unused())
    }
    async fn batch_update_client_keys(
        &self,
        _: gateway_admin::model::client_keys::BatchUpdateClientKeys,
        _: gateway_admin::ports::store::BatchClientKeyProfileMerger,
        _: &gateway_admin::model::MutationContext,
    ) -> AdminStoreResult<gateway_admin::model::Revision> {
        Err(unused())
    }

    async fn reset_client_key_budget(
        &self,
        _: gateway_admin::model::client_keys::ResetClientKeyBudget,
        _: &MutationContext,
    ) -> AdminStoreResult<()> {
        Ok(())
    }
    async fn list_client_keys(&self, query: ClientKeyListQuery) -> AdminStoreResult<ClientKeyPage> {
        assert_eq!(query.page_size.get(), u16::MAX);
        Ok(ClientKeyPage {
            config_revision: Revision::new(1).expect("revision"),
            items: Vec::new(),
            total: 0,
            next_cursor: None,
        })
    }

    async fn reveal_client_key(
        &self,
        _: &ClientApiKeyId,
    ) -> AdminStoreResult<Option<ClientKeySecret>> {
        Err(unused())
    }

    async fn create_client_key(
        &self,
        _: NewClientKey,
        _: &MutationContext,
    ) -> AdminStoreResult<(Revision, ClientKeyRecord)> {
        Err(unused())
    }

    async fn update_client_key(
        &self,
        _: UpdateClientKey,
        _: &MutationContext,
    ) -> AdminStoreResult<(Revision, ClientKeyRecord)> {
        Err(unused())
    }

    async fn set_client_key_enabled(
        &self,
        _: SetClientKeyEnabled,
        _: &MutationContext,
    ) -> AdminStoreResult<(Revision, ClientKeyRecord)> {
        Err(unused())
    }

    async fn delete_client_key(
        &self,
        _: DeleteClientKey,
        _: &MutationContext,
    ) -> AdminStoreResult<Revision> {
        Err(unused())
    }
}

#[tokio::test]
async fn client_key_cursor_should_reject_value_that_does_not_match_sort() {
    let services = super::AdminHarness::new()
        .client_keys(std::sync::Arc::new(UnusedClientKeyStore))
        .build()
        .await;
    let sort = ClientKeySort {
        field: ClientKeySortField::Name,
        direction: SortDirection::Asc,
    };
    let error = services
        .client_keys()
        .list(ClientKeyListQuery {
            cursor: Some(ClientKeyCursor {
                sort,
                value: ClientKeyCursorValue::Enabled(true),
                id: ClientApiKeyId::new("key_cursor").expect("key ID"),
            }),
            page_size: ClientKeyPageSize::new(50).expect("page size"),
            search: None,
            sort,
        })
        .await
        .expect_err("mismatched cursor must fail");

    assert_eq!(error.kind(), AdminErrorKind::Invalid);
}

#[tokio::test]
async fn client_key_list_should_forward_the_full_nonzero_u16_page_size() {
    let services = super::AdminHarness::new()
        .client_keys(std::sync::Arc::new(UnusedClientKeyStore))
        .build()
        .await;
    let page = services
        .client_keys()
        .list(ClientKeyListQuery {
            cursor: None,
            page_size: ClientKeyPageSize::new(u16::MAX).expect("maximum page size"),
            search: None,
            sort: ClientKeySort {
                field: ClientKeySortField::CreatedAt,
                direction: SortDirection::Desc,
            },
        })
        .await
        .expect("maximum page size should reach store");

    assert_eq!(page.total, 0);
}

fn unused() -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Unavailable,
        "client key",
        "unused in this test",
    )
}

#[test]
fn budget_alignment_accepts_only_current_account_wide_day_and_week_windows() {
    use chrono::{DateTime, Duration, Utc};
    use gateway_admin::model::{
        client_keys::{ClientKeyBudgetReference, ClientKeyBudgetWindowPeriod},
        provider_credentials::{ProviderQuotaWindow, QuotaLocalUsageAttribution},
    };

    let now = DateTime::parse_from_rfc3339("2026-01-15T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let window = |seconds| ProviderQuotaWindow {
        key: "account-window".to_owned(),
        group: "account".to_owned(),
        label: "Account window".to_owned(),
        limit_id: None,
        limit_name: None,
        role: None,
        local_usage_attribution: QuotaLocalUsageAttribution::AccountWide,
        window_seconds: seconds,
        used_percent: Some(50.0),
        reset_at: Some(now + Duration::hours(1)),
        limit_reached: false,
        local_usage: None,
        provider_data: None,
    };

    for (seconds, period) in [
        (86_400, ClientKeyBudgetWindowPeriod::Daily),
        (604_800, ClientKeyBudgetWindowPeriod::Weekly),
    ] {
        let reference = ClientKeyBudgetReference::from_current_window(window(Some(seconds)), now)
            .expect("current account-wide day/week is eligible");
        assert_eq!(reference.period, period);
        assert_eq!(reference.key, "account-window");
        assert_eq!(reference.bounds.ends_at, now + Duration::hours(1));
        assert_eq!(
            reference.bounds.ends_at - reference.bounds.starts_at,
            Duration::seconds(i64::try_from(seconds).unwrap())
        );
        assert!(reference.bounds.resume_calendar);
    }

    // Five-hour limits must never be presented as a daily budget reference.
    for seconds in [Some(18_000), Some(3_600), Some(0), None] {
        assert!(ClientKeyBudgetReference::from_current_window(window(seconds), now).is_none());
    }
    let mut model_specific = window(Some(86_400));
    model_specific.local_usage_attribution = QuotaLocalUsageAttribution::Unavailable;
    assert!(ClientKeyBudgetReference::from_current_window(model_specific, now).is_none());

    for reset_at in [
        None,
        Some(now),
        Some(now - Duration::seconds(1)),
        Some(now + Duration::days(2)),
    ] {
        let mut outside_current_window = window(Some(86_400));
        outside_current_window.reset_at = reset_at;
        assert!(
            ClientKeyBudgetReference::from_current_window(outside_current_window, now).is_none()
        );
    }
}

#[tokio::test]
async fn clearing_an_unregistered_provider_profile_is_rejected_before_writing() {
    let services = super::AdminHarness::new().build().await;
    let command = UpdateClientKey {
        id: ClientApiKeyId::new("key_existing").unwrap(),
        name: "test".to_owned(),
        label: None,
        group_ids: Vec::new(),
        limits: gateway_core::policy::RateLimits::unlimited(),
        daily_limit_usd: None,
        weekly_limit_usd: None,
        peer_sharing: None,
        request_profile_override_updates: [(
            gateway_core::routing::ProviderKind::new("unregistered").unwrap(),
            None,
        )]
        .into(),
    };
    assert_eq!(
        services
            .client_keys()
            .update(
                &MutationContext {
                    actor: gateway_admin::model::MutationActor::System,
                    request_id: "clear-unregistered-profile".into()
                },
                command
            )
            .await
            .unwrap_err()
            .kind(),
        AdminErrorKind::Invalid
    );
}
