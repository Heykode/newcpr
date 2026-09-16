use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use gateway_admin::{
    model::{
        MutationContext, Revision,
        egress::{ProviderEgressMutation, ReplaceProviderEgress, SetProviderAccountEgress},
    },
    ports::store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult, ProviderEgressStore},
};
use gateway_api::admin::egress::{AccountEgressRequest, ExpandEgressRequest, ReplaceEgressRequest};
use gateway_core::{
    account::ProviderAccountId,
    provider_ports::egress::{EgressMode, ProviderEgressAddress, ProviderEgressConfig},
};
use serde_json::{Value, json};
use tower::ServiceExt as _;

use super::{AdminTestFixture, AdminTestState};

struct MemoryEgress {
    config: Mutex<ProviderEgressConfig>,
    mutations: AtomicUsize,
}

impl MemoryEgress {
    fn new(config: ProviderEgressConfig) -> Self {
        Self {
            config: Mutex::new(config),
            mutations: AtomicUsize::new(0),
        }
    }

    fn mutate(
        &self,
        revision: Revision,
        apply: impl FnOnce(&mut ProviderEgressConfig) -> AdminStoreResult<()>,
    ) -> AdminStoreResult<ProviderEgressMutation> {
        let mut stored = self.config.lock().unwrap();
        if revision.get() != stored.revision {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::StaleRevision,
                "provider IPv6 egress",
                "stale revision",
            ));
        }
        let mut next = stored.clone();
        apply(&mut next)?;
        next.revision += 1;
        *stored = next.clone();
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Ok(ProviderEgressMutation {
            // Runtime configuration revision is independent of the egress CAS revision.
            config_revision: Revision::new(next.revision + 100).unwrap(),
            config: next,
        })
    }
}

#[async_trait]
impl ProviderEgressStore for MemoryEgress {
    async fn load(&self) -> AdminStoreResult<ProviderEgressConfig> {
        Ok(self.config.lock().unwrap().clone())
    }

    async fn replace(
        &self,
        command: ReplaceProviderEgress,
        _: &MutationContext,
    ) -> AdminStoreResult<ProviderEgressMutation> {
        self.mutate(command.expected_revision, |config| {
            config.default_mode = command.default_mode;
            config.addresses = command.addresses;
            Ok(())
        })
    }

    async fn set_account(
        &self,
        command: SetProviderAccountEgress,
        _: &MutationContext,
    ) -> AdminStoreResult<ProviderEgressMutation> {
        self.mutate(command.expected_revision, |config| {
            let mode = config
                .account_overrides
                .get_mut(&command.account_id)
                .ok_or_else(|| {
                    AdminStoreError::new(
                        AdminStoreErrorKind::NotFound,
                        "provider account IPv6 egress",
                        "missing account",
                    )
                })?;
            *mode = command.mode;
            Ok(())
        })
    }
}

fn test_config() -> ProviderEgressConfig {
    ProviderEgressConfig {
        revision: 1,
        default_mode: EgressMode::Unchanged,
        addresses: vec![ProviderEgressAddress {
            id: "ipv6_one".to_owned(),
            address: "2001:db8::1".parse().unwrap(),
            enabled: false,
        }],
        account_overrides: BTreeMap::from([
            (ProviderAccountId::new("acct_test").unwrap(), None),
            (
                ProviderAccountId::new("acct_explicit").unwrap(),
                Some(EgressMode::Unchanged),
            ),
            (ProviderAccountId::new("acct_unbound").unwrap(), None),
        ]),
        fixed_bindings: BTreeMap::from([(
            ProviderAccountId::new("acct_test").unwrap(),
            "2001:db8::1".parse().unwrap(),
        )]),
    }
}

async fn request(
    fixture: &AdminTestFixture,
    path: &str,
    body: Option<Value>,
    authenticated: bool,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .uri(path)
        .method(if body.is_some() { "POST" } else { "GET" })
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-request-id", "req_egress_tests");
    if authenticated {
        builder = builder.header(header::COOKIE, "cpr_admin_session=valid-session");
    }
    let response = gateway_api::admin::egress::router::<AdminTestState>()
        .with_state(fixture.state())
        .oneshot(
            builder
                .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[test]
fn account_egress_distinguishes_inheritance_from_explicit_unchanged() {
    for (extra, expected) in [
        (json!({}), None),
        (json!({"mode": null}), None),
        (json!({"mode": ""}), None),
        (json!({"mode": "  "}), None),
        (json!({"mode": "unchanged"}), Some(EgressMode::Unchanged)),
        (
            json!({"mode": "fixed_ipv6_reuse"}),
            Some(EgressMode::FixedIpv6Reuse),
        ),
    ] {
        let mut body = extra;
        body["accountId"] = json!("acct_test");
        body["revision"] = json!(1);
        let command = serde_json::from_value::<AccountEgressRequest>(body)
            .unwrap()
            .into_command()
            .unwrap_or_else(|_| panic!("valid account egress request"));
        assert_eq!(command.mode, expected);
    }
}

#[test]
fn ipv6_ranges_are_bounded_normalized_disabled_and_unicast_only() {
    let range = ExpandEgressRequest {
        start: "2001:db8::".to_owned(),
        end: "2001:db8::fff".to_owned(),
    }
    .expand()
    .unwrap_or_else(|_| panic!("valid bounded IPv6 range"));
    assert_eq!(range.len(), 4096);
    assert!(range.iter().all(|address| !address.enabled));
    assert_eq!(range[0].address, "2001:db8::");
    for (start, end) in [
        ("2001:db8::", "2001:db8::1000"),
        ("2001:db8::2", "2001:db8::1"),
        ("::", "::"),
        ("::1", "::1"),
        ("::ffff:192.0.2.1", "::ffff:192.0.2.1"),
        ("fe80::1", "fe80::1"),
        ("ff02::1", "ff02::1"),
        ("fec0::1", "fec0::1"),
        ("192.0.2.1", "192.0.2.1"),
        ("fe80::1%lo0", "fe80::1%lo0"),
    ] {
        assert!(
            ExpandEgressRequest {
                start: start.into(),
                end: end.into()
            }
            .expand()
            .is_err()
        );
    }
}

#[test]
fn pool_request_defaults_new_addresses_to_disabled_and_rejects_invalid_modes() {
    let body = json!({
        "revision": 1,
        "defaultMode": "unchanged",
        "addresses": [{"id": "ipv6_one", "address": "2001:db8::1"}]
    });
    let command = serde_json::from_value::<ReplaceEgressRequest>(body.clone())
        .unwrap()
        .into_command()
        .unwrap_or_else(|_| panic!("valid IPv6 pool request"));
    assert_eq!(command.default_mode, EgressMode::Unchanged);
    assert!(!command.addresses[0].enabled);
    for (field, value) in [
        ("revision", json!(0)),
        ("revision", json!(u64::MAX)),
        ("defaultMode", json!("")),
        ("defaultMode", json!("fallback_ipv4")),
        ("addresses", json!([{"id": "one", "address": "::1"}])),
    ] {
        let mut invalid = body.clone();
        invalid[field] = value;
        assert!(
            serde_json::from_value::<ReplaceEgressRequest>(invalid)
                .unwrap()
                .into_command()
                .is_err()
        );
    }
    let mut invalid = body;
    invalid["unknown"] = json!(true);
    assert!(serde_json::from_value::<ReplaceEgressRequest>(invalid).is_err());
}

#[tokio::test]
async fn all_ipv6_routes_require_admin_authentication_before_parsing_input() {
    let fixture = AdminTestFixture::new().await;
    for (path, body) in [
        ("/api/admin/ipv6-egress", None),
        (
            "/api/admin/ipv6-egress/expand",
            Some(json!({"start":"2001:db8::1","end":"2001:db8::2"})),
        ),
        ("/api/admin/ipv6-egress/update", Some(json!({}))),
        ("/api/admin/ipv6-egress/account", Some(json!({}))),
    ] {
        assert_eq!(
            request(&fixture, path, body, false).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
}

#[tokio::test]
async fn ipv6_authenticated_read_and_expand_preserve_snapshot_and_unbound_accounts() {
    let mut config = test_config();
    config.default_mode = EgressMode::FixedIpv6Reuse;
    config.addresses[0].enabled = true;
    let store = Arc::new(MemoryEgress::new(config.clone()));
    let fixture = AdminTestFixture::with_egress(store.clone()).await;
    fixture.auth.insert_session("valid-session");

    let (status, loaded) = request(&fixture, "/api/admin/ipv6-egress", None, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        loaded["data"],
        json!({
            "revision": 1,
            "defaultMode": "fixed_ipv6_reuse",
            "addresses": [{"id": "ipv6_one", "address": "2001:db8::1", "enabled": true}],
            "accountOverrides": {"acct_test": null, "acct_explicit": "unchanged", "acct_unbound": null},
            "fixedBindings": {"acct_test": "2001:db8::1"}
        })
    );

    let (status, expanded) = request(
        &fixture,
        "/api/admin/ipv6-egress/expand",
        Some(json!({"start": "2001:0db8:0:0:0:0:0:2", "end": "2001:db8::3"})),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        expanded["data"],
        json!([
            {"id": "ipv6_20010db8000000000000000000000002", "address": "2001:db8::2", "enabled": false},
            {"id": "ipv6_20010db8000000000000000000000003", "address": "2001:db8::3", "enabled": false}
        ])
    );
    let (_, reloaded) = request(&fixture, "/api/admin/ipv6-egress", None, true).await;
    assert_eq!(reloaded["data"], loaded["data"]);
    assert_eq!(*store.config.lock().unwrap(), config);
    assert_eq!(store.mutations.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ipv6_pool_json_roundtrip_rejects_stale_revision_without_partial_changes() {
    let store = Arc::new(MemoryEgress::new(test_config()));
    let fixture = AdminTestFixture::with_egress(store.clone()).await;
    fixture.auth.insert_session("valid-session");

    let (status, updated) = request(
        &fixture,
        "/api/admin/ipv6-egress/update",
        Some(json!({
            "revision": 1,
            "defaultMode": "random_ipv6_fresh",
            "addresses": [
                {"id": "ipv6_two", "address": "2001:0db8:0:0:0:0:0:2"},
                {"id": "ipv6_one", "address": "2001:db8::1", "enabled": false}
            ]
        })),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        updated["data"],
        json!({
            "configRevision": 102,
            "config": {
                "revision": 2,
                "defaultMode": "random_ipv6_fresh",
                "addresses": [
                    {"id": "ipv6_two", "address": "2001:db8::2", "enabled": false},
                    {"id": "ipv6_one", "address": "2001:db8::1", "enabled": false}
                ],
                "accountOverrides": {"acct_test": null, "acct_explicit": "unchanged", "acct_unbound": null},
                "fixedBindings": {"acct_test": "2001:db8::1"}
            }
        })
    );

    let (status, rejected) = request(
        &fixture,
        "/api/admin/ipv6-egress/update",
        Some(json!({"revision": 1, "defaultMode": "unchanged", "addresses": []})),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(rejected["data"].is_null());
    let (status, loaded) = request(&fixture, "/api/admin/ipv6-egress", None, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(loaded["data"], updated["data"]["config"]);
    assert_eq!(store.mutations.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ipv6_account_json_roundtrip_clears_overrides_but_keeps_explicit_unchanged() {
    let mut config = test_config();
    config.default_mode = EgressMode::FixedIpv6Fresh;
    let store = Arc::new(MemoryEgress::new(config));
    let fixture = AdminTestFixture::with_egress(store.clone()).await;
    fixture.auth.insert_session("valid-session");
    let mut revision = 1;

    for (extra, expected) in [
        (json!({"mode": "unchanged"}), json!("unchanged")),
        (json!({"mode": null}), Value::Null),
        (
            json!({"mode": "fixed_ipv6_reuse"}),
            json!("fixed_ipv6_reuse"),
        ),
        (json!({}), Value::Null),
        (
            json!({"mode": "random_ipv6_reuse"}),
            json!("random_ipv6_reuse"),
        ),
        (json!({"mode": ""}), Value::Null),
        (
            json!({"mode": "random_ipv6_fresh"}),
            json!("random_ipv6_fresh"),
        ),
        (json!({"mode": "  "}), Value::Null),
        (json!({"mode": "unchanged"}), json!("unchanged")),
    ] {
        let mut body = extra;
        body["accountId"] = json!("acct_test");
        body["revision"] = json!(revision);
        let (status, updated) =
            request(&fixture, "/api/admin/ipv6-egress/account", Some(body), true).await;
        assert_eq!(status, StatusCode::OK);
        revision += 1;
        assert_eq!(updated["data"]["configRevision"], revision + 100);
        assert_eq!(updated["data"]["config"]["revision"], revision);
        assert_eq!(
            updated["data"]["config"]["accountOverrides"]["acct_test"],
            expected
        );
        assert_eq!(updated["data"]["config"]["defaultMode"], "fixed_ipv6_fresh");
        assert_eq!(
            updated["data"]["config"]["fixedBindings"],
            json!({"acct_test": "2001:db8::1"})
        );
        let (status, loaded) = request(&fixture, "/api/admin/ipv6-egress", None, true).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(loaded["data"], updated["data"]["config"]);
    }

    let (_, before) = request(&fixture, "/api/admin/ipv6-egress", None, true).await;
    let (status, _) = request(
        &fixture,
        "/api/admin/ipv6-egress/account",
        Some(json!({"accountId": "acct_test", "revision": revision - 1, "mode": null})),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = request(
        &fixture,
        "/api/admin/ipv6-egress/account",
        Some(json!({"accountId": "acct_missing", "revision": revision, "mode": "unchanged"})),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, after) = request(&fixture, "/api/admin/ipv6-egress", None, true).await;
    assert_eq!(after["data"], before["data"]);
    assert_eq!(after["data"]["accountOverrides"]["acct_test"], "unchanged");
    assert_eq!(store.mutations.load(Ordering::SeqCst), 9);
}

#[tokio::test]
async fn ipv6_invalid_pool_and_failed_source_validation_do_not_commit() {
    let config = test_config();
    let store = Arc::new(MemoryEgress::new(config.clone()));
    let fixture = AdminTestFixture::with_egress(store.clone()).await;
    fixture.auth.insert_session("valid-session");

    for addresses in [
        json!([{"id": "ipv6_one", "address": "2001:db8::1", "enabled": true}]),
        json!([
            {"id": "ipv6_one", "address": "2001:db8::1"},
            {"id": "ipv6_one", "address": "2001:db8::2"}
        ]),
        json!([
            {"id": "ipv6_one", "address": "2001:db8::1"},
            {"id": "ipv6_two", "address": "2001:0db8:0:0:0:0:0:1"}
        ]),
        json!([{"id": "", "address": "2001:db8::1"}]),
        json!([{"id": "ipv6_bad", "address": "fe80::1"}]),
    ] {
        let (status, _) = request(
            &fixture,
            "/api/admin/ipv6-egress/update",
            Some(json!({"revision": 1, "defaultMode": "unchanged", "addresses": addresses})),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(*store.config.lock().unwrap(), config);
    }
    assert_eq!(store.mutations.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ipv6_inherited_source_validation_cannot_prevent_explicit_disable() {
    let mut config = test_config();
    config.default_mode = EgressMode::FixedIpv6Reuse;
    config.addresses[0].enabled = true;
    let store = Arc::new(MemoryEgress::new(config.clone()));
    let fixture = AdminTestFixture::with_egress(store.clone()).await;
    fixture.auth.insert_session("valid-session");

    // The fixture provider rejects source validation: inheritance must fail before commit.
    for mut body in [json!({}), json!({"mode": null}), json!({"mode": ""})] {
        body["accountId"] = json!("acct_test");
        body["revision"] = json!(1);
        let (status, _) =
            request(&fixture, "/api/admin/ipv6-egress/account", Some(body), true).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(*store.config.lock().unwrap(), config);
    }
    assert_eq!(store.mutations.load(Ordering::SeqCst), 0);

    let (status, updated) = request(
        &fixture,
        "/api/admin/ipv6-egress/account",
        Some(json!({"accountId": "acct_test", "revision": 1, "mode": "unchanged"})),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        updated["data"]["config"]["accountOverrides"]["acct_test"],
        "unchanged"
    );
    assert_eq!(updated["data"]["config"]["defaultMode"], "fixed_ipv6_reuse");
    assert_eq!(updated["data"]["config"]["addresses"][0]["enabled"], true);
    assert_eq!(store.mutations.load(Ordering::SeqCst), 1);
}
