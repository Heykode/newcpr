use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use gateway_admin::model::mihomo::{CountryFilter, CountryFilterMode, MihomoCommand};
use gateway_core::{
    account::OutboundProxy,
    provider_ports::session_proxy::{
        RequestProxySource, SessionProxyError, SessionProxyOutcome, SessionProxyPool,
    },
};
use serde_json::json;

use super::{
    dynamic, files,
    kernel::Kernel,
    pool::{ExitPools, PoolState, lock},
    sources,
    state::{Saved, node_id, normalize_country},
};

fn proxy(port: u16) -> OutboundProxy {
    OutboundProxy::parse(&format!("http://user:secret@127.0.0.1:{port}")).unwrap()
}

#[test]
fn qualification_requires_structured_auth_rejection() {
    use super::warm::auth_rejection;
    for body in [
        br#"{"detail":"Not authenticated"}"#.as_slice(),
        br#"{"error":{"message":"Missing token"}}"#,
        br#"{"error":"unauthorized"}"#,
    ] {
        assert_eq!(auth_rejection(body), Ok(()));
    }
    for body in [
        b"<html>challenge</html>".as_slice(),
        br#"{"error":null}"#,
        br#"{"error":{}}"#,
        b"{}",
    ] {
        assert!(auth_rejection(body).is_err());
    }
}

#[tokio::test]
#[ignore = "downloads the official kernel; run only in an isolated Linux network namespace"]
async fn official_kernel_relays_reloads_rejects_and_stops() {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};
    assert!(Kernel::supported());
    let root = tempfile::tempdir().unwrap();
    files::prepare_dir(root.path()).unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(120))
        .build()
        .unwrap();
    let mut kernel = Kernel::new(root.path().to_owned());
    kernel.install(&client).await.unwrap();
    let upstream = MockServer::start().await;
    Mock::given(path("/relay-fixture"))
        .respond_with(ResponseTemplate::new(200).set_body_string("through-selected-node"))
        .mount(&upstream)
        .await;
    let node = json!({"name":"fixture-exit","type":"direct"})
        .as_object()
        .unwrap()
        .clone();
    let mut saved = Saved {
        nodes: vec![node.clone()],
        secret: files::secret().unwrap(),
        ..Default::default()
    };
    let config = kernel.config(&saved).unwrap();
    let path = kernel.validate(&config).await.unwrap();
    kernel.start(&path, &saved.secret, &client).await.unwrap();
    let exit = reqwest::Client::builder()
        .no_proxy()
        .proxy(
            reqwest::Proxy::all(format!(
                "http://127.0.0.1:{}",
                kernel.ports[&node_id(&node)]
            ))
            .unwrap(),
        )
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let target = format!("{}/relay-fixture", upstream.uri());
    assert_eq!(
        exit.get(&target)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "through-selected-node"
    );
    let mut invalid = config.clone();
    invalid["rules"] = json!(["MATCH,missing-node"]);
    assert!(kernel.validate(&invalid).await.is_err());
    assert_eq!(
        exit.get(&target)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "through-selected-node"
    );
    saved
        .disabled
        .insert("fixture-exit".into(), "disabled".into());
    let disabled = kernel.config(&saved).unwrap();
    super::kernel::reload(&client, &disabled, &saved.secret)
        .await
        .unwrap();
    let rejected = exit.get(&target).send().await;
    assert!(!rejected.is_ok_and(|r| r.status().is_success()));
    saved.disabled.clear();
    super::kernel::reload(&client, &kernel.config(&saved).unwrap(), &saved.secret)
        .await
        .unwrap();
    assert_eq!(
        exit.get(&target)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "through-selected-node"
    );
    kernel.stop().await;
    assert!(!kernel.running());
    assert!(exit.get(&target).send().await.is_err());
    assert_eq!(upstream.received_requests().await.unwrap().len(), 3);
    let config = kernel.config(&saved).unwrap();
    let path = kernel.validate(&config).await.unwrap();
    kernel.start(&path, &saved.secret, &client).await.unwrap();
    drop(kernel);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:3101").await {
                drop(listener);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("kernel must stop when its owner disappears");
}

fn populate(pools: &ExitPools, dynamic: bool) {
    let mut state = lock(&pools.managed);
    let now = Instant::now();
    state.replace(
        vec![
            ("a".into(), proxy(19000), dynamic, !dynamic),
            ("b".into(), proxy(19001), false, false),
        ],
        true,
        now,
    );
    for id in ["a", "b"] {
        qualify(&mut state, id, Ok(()), now);
    }
}

fn qualify(state: &mut PoolState, id: &str, result: Result<(), &'static str>, now: Instant) {
    let h = &state.exits[id];
    state.qualify(id, h.generation, h.revision, result, &[result.is_ok()], now);
}

#[test]
fn diagnostic_exit_retains_geography_and_accepts_ip_only_fallback() {
    let exit = super::diagnostics::parse_exit(
        br#"{"status":"success","query":"203.0.113.1","countryCode":"US","country":"United States","region":"CA","regionName":"California","city":"Los Angeles"}"#,
        true,
    ).unwrap();
    assert_eq!(exit.country_name.as_deref(), Some("United States"));
    assert_eq!(exit.region.as_deref(), Some("California"));
    assert_eq!(exit.city.as_deref(), Some("Los Angeles"));
    let fallback = super::diagnostics::parse_exit(br#"{"ip":"2001:db8::1"}"#, false).unwrap();
    assert_eq!(fallback.exit_ip.as_deref(), Some("2001:db8::1"));
    assert!(fallback.country_code.is_none());
    assert!(super::diagnostics::parse_exit(br#"{"ip":"not-an-ip"}"#, false).is_none());
    assert!(
        super::diagnostics::parse_exit(br#"{"status":"fail","query":"203.0.113.1"}"#, true)
            .is_none()
    );
}

#[test]
fn repeated_probe_failures_preserve_cooldown_and_two_successes_recover() {
    let mut state = PoolState::default();
    let now = Instant::now();
    state.replace(vec![("a".into(), proxy(19000), false, true)], true, now);
    let h = &state.exits["a"];
    state.qualify(
        "a",
        h.generation,
        h.revision,
        Err("network_error"),
        &[false, false],
        now,
    );
    let h = &state.exits["a"];
    assert_eq!(h.failures, 2);
    assert!(h.cooling(now + Duration::from_secs(59)));
    assert!(!h.ready(now));
    let later = now + Duration::from_secs(61);
    state.qualify("a", h.generation, h.revision, Ok(()), &[true, true], later);
    assert!(state.exits["a"].ready(later));
    assert_eq!(state.exits["a"].failures, 0);
}

#[test]
fn dynamic_formats_preserve_credentials_and_deduplicate() {
    let expected = dynamic::parse("http://user:p%40ss%3Aword@proxy.example:3000")
        .unwrap()
        .normalized;
    for value in [
        "proxy.example:3000:user:p@ss:word",
        "user:p@ss:word:proxy.example:3000",
        "user:p%40ss%3Aword@proxy.example:3000",
        "proxy.example:3000@user:p%40ss%3Aword",
    ] {
        assert_eq!(
            dynamic::parse(value).unwrap().normalized,
            expected,
            "{value}"
        );
    }
    assert_eq!(
        dynamic::normalize(&[expected.clone(), expected])
            .unwrap()
            .len(),
        1
    );
    let ipv6 = dynamic::parse("http://user:secret@[2001:db8::1]:8080").unwrap();
    assert_eq!(ipv6.node()["server"], "2001:db8::1");
    assert_eq!(
        dynamic::parse("https://u:p@host:443").unwrap().node()["tls"],
        true
    );
    assert_eq!(
        dynamic::parse("socks5h://u:p@host:1080").unwrap().node()["type"],
        "socks5"
    );
}

#[test]
fn invalid_dynamic_inputs_never_echo_credentials() {
    for value in [
        "ftp://private-secret@bad",
        "host:0:user:private-secret",
        "http://u:private-secret@host:12/path",
        "http://u:private-secret%QQ@host:12",
    ] {
        let error = dynamic::parse(value).err().expect("reject").to_string();
        assert!(!error.contains("private-secret"));
    }
}

#[test]
fn subscription_imports_only_proxies_and_identity_ignores_labels() {
    let cache = sources::parse_subscription(
        br#"
mixed-port: 9999
allow-lan: true
external-controller: 0.0.0.0:9998
rules: [MATCH,DIRECT]
proxies:
  - {name: first, type: http, server: host, port: 8000}
  - {name: second, type: http, server: host, port: 8000}
"#,
    )
    .unwrap();
    assert_eq!(cache.nodes.len(), 1);
    let node = &cache.nodes[0];
    let mut renamed = node.clone();
    renamed.insert("name".into(), "another".into());
    assert_eq!(node_id(node), node_id(&renamed));
    renamed.insert("password".into(), "changed".into());
    assert_ne!(node_id(node), node_id(&renamed));
    assert!(!node.contains_key("rules"));
}

#[test]
fn complete_identity_is_stable_across_nested_json_key_order() {
    let first: serde_json::Value = serde_json::from_str(
        r#"{"type":"http","port":8000,"server":"host","headers":{"A":"a","B":"b"},"name":"first"}"#,
    )
    .unwrap();
    let second: serde_json::Value = serde_json::from_str(r#"{"headers":{"B":"b","A":"a"},"name":"second","server":"host","port":8000,"type":"http"}"#).unwrap();
    assert_eq!(
        node_id(first.as_object().unwrap()),
        node_id(second.as_object().unwrap())
    );
}

#[test]
fn country_policy_preserves_dynamic_provider_semantics() {
    let filter = normalize_country(CountryFilter {
        mode: CountryFilterMode::Include,
        codes: vec![" us ".into(), "US".into()],
        allow_unknown: false,
        dynamic_provider_managed: false,
    })
    .unwrap();
    assert_eq!(filter.codes, ["US"]);
    let mut saved = Saved {
        country_filter: filter,
        ..Default::default()
    };
    let node = dynamic::parse("host:8000:u:p").unwrap().node();
    assert!(!saved.country_allowed(&node, true));
    saved.country_filter.dynamic_provider_managed = true;
    assert!(saved.country_allowed(&node, true));
    assert!(!saved.country_allowed(&node, false));
    for code in ["UK", "ZZ", "EU", "001"] {
        assert!(
            normalize_country(CountryFilter {
                codes: vec![code.into()],
                ..Default::default()
            })
            .is_err()
        );
    }
}

#[test]
fn node_ports_are_stable_only_within_process_and_excluded_nodes_do_not_allocate() {
    let dir = tempfile::tempdir().unwrap();
    let a = dynamic::parse("host-a:8000:u:p").unwrap().node();
    let b = dynamic::parse("host-b:8000:u:p").unwrap().node();
    let mut kernel = Kernel::new(dir.path().into());
    let mut saved = Saved {
        nodes: vec![a.clone()],
        ..Default::default()
    };
    kernel.config(&saved).unwrap();
    saved.nodes = vec![b.clone()];
    let config = kernel.config(&saved).unwrap();
    assert_eq!(kernel.ports[&node_id(&a)], 19000);
    assert_eq!(kernel.ports[&node_id(&b)], 19001);
    assert!(
        config["listeners"]
            .as_array()
            .unwrap()
            .iter()
            .any(|listener| listener["port"] == 19000 && listener["proxy"] == "REJECT")
    );
    let mut restarted = Kernel::new(dir.path().into());
    restarted.config(&saved).unwrap();
    assert_eq!(restarted.ports[&node_id(&b)], 19000);
    saved
        .disabled
        .insert(b["name"].as_str().unwrap().into(), "disabled".into());
    let mut excluded = Kernel::new(dir.path().into());
    excluded.config(&saved).unwrap();
    assert!(excluded.ports.is_empty());
    assert!(serde_json::to_value(saved).unwrap().get("ports").is_none());
}

#[test]
fn empty_or_cold_pool_fails_closed_without_allocating_a_session() {
    let pools = ExitPools::default();
    assert_eq!(
        pools
            .acquire(
                RequestProxySource::Account,
                gateway_core::account::ResponsesUpstream::Excel,
                "x",
                false
            )
            .err(),
        Some(SessionProxyError::Unavailable)
    );
    assert_eq!(
        pools
            .acquire(
                RequestProxySource::Mihomo,
                gateway_core::account::ResponsesUpstream::Excel,
                "x",
                false
            )
            .err(),
        Some(SessionProxyError::Unavailable)
    );
    lock(&pools.managed).replace(
        vec![("a".into(), proxy(19000), false, true)],
        true,
        Instant::now(),
    );
    assert_eq!(
        pools
            .acquire(
                RequestProxySource::Mihomo,
                gateway_core::account::ResponsesUpstream::Excel,
                "x",
                false
            )
            .err(),
        Some(SessionProxyError::Warming)
    );
    assert!(lock(&pools.managed).loads().is_empty());
}

#[test]
fn concurrent_session_references_release_only_on_last_clone() {
    let pools = ExitPools::default();
    populate(&pools, false);
    let one = pools
        .acquire(
            RequestProxySource::Mihomo,
            gateway_core::account::ResponsesUpstream::Excel,
            "account/key/session",
            false,
        )
        .unwrap();
    let clone = Arc::clone(&one);
    let two = pools
        .acquire(
            RequestProxySource::Mihomo,
            gateway_core::account::ResponsesUpstream::Excel,
            "account/key/session",
            false,
        )
        .unwrap();
    assert_eq!(one.node_id(), two.node_id());
    assert_eq!(lock(&pools.managed).loads()[one.node_id()].0, 2);
    let id = one.node_id().to_owned();
    drop(one);
    assert_eq!(lock(&pools.managed).loads()[&id].0, 2);
    drop(clone);
    assert_eq!(lock(&pools.managed).loads()[&id].0, 1);
    drop(two);
    assert_eq!(lock(&pools.managed).loads()[&id].0, 0);
}

#[test]
fn codex_and_excel_health_and_sessions_are_independent() {
    use gateway_core::account::ResponsesUpstream;
    let pools = ExitPools::default();
    populate(&pools, false);
    let now = Instant::now();
    for pool in [&pools.codex_managed, &pools.codex_regular] {
        let mut state = lock(pool);
        state.replace(
            vec![("codex-only".into(), proxy(19002), false, true)],
            true,
            now,
        );
        qualify(&mut state, "codex-only", Ok(()), now);
    }
    let codex = pools
        .acquire(
            RequestProxySource::Mihomo,
            ResponsesUpstream::Codex,
            "same-account/session",
            false,
        )
        .unwrap();
    let excel = pools
        .acquire(
            RequestProxySource::Mihomo,
            ResponsesUpstream::Excel,
            "same-account/session",
            false,
        )
        .unwrap();
    assert_eq!(codex.node_id(), "codex-only");
    assert_ne!(codex.node_id(), excel.node_id());
    excel.report(SessionProxyOutcome::StreamFailure);
    let next = pools
        .acquire(
            RequestProxySource::Mihomo,
            ResponsesUpstream::Codex,
            "same-account/session",
            false,
        )
        .unwrap();
    assert_eq!(next.node_id(), codex.node_id());
    codex.report(SessionProxyOutcome::NetworkFailure);
    assert!(
        pools
            .acquire(
                RequestProxySource::Mihomo,
                ResponsesUpstream::Codex,
                "same-account/session",
                false
            )
            .is_err()
    );
    let regular = pools
        .acquire(
            RequestProxySource::ProxyPool,
            ResponsesUpstream::Codex,
            "same-account/session",
            false,
        )
        .unwrap();
    assert_eq!(regular.node_id(), "codex-only");
    drop((codex, next, excel, regular));
    assert_eq!(lock(&pools.codex_managed).loads()["codex-only"].0, 0);
}

#[test]
fn failed_session_drains_then_rebinds_without_poisoning_other_exit() {
    let pools = ExitPools::default();
    populate(&pools, false);
    let one = pools
        .acquire(
            RequestProxySource::Mihomo,
            gateway_core::account::ResponsesUpstream::Excel,
            "session",
            false,
        )
        .unwrap();
    let id = one.node_id().to_owned();
    one.report(SessionProxyOutcome::StreamFailure);
    assert_eq!(
        pools
            .acquire(
                RequestProxySource::Mihomo,
                gateway_core::account::ResponsesUpstream::Excel,
                "session",
                false
            )
            .err(),
        Some(SessionProxyError::Draining)
    );
    drop(one);
    let two = pools
        .acquire(
            RequestProxySource::Mihomo,
            gateway_core::account::ResponsesUpstream::Excel,
            "session",
            false,
        )
        .unwrap();
    assert_ne!(two.node_id(), id);
    assert!(!lock(&pools.managed).exits[two.node_id()].cooling(Instant::now()));
}

#[test]
fn transient_bindings_do_not_accumulate_and_pools_are_isolated() {
    let pools = ExitPools::default();
    populate(&pools, false);
    for n in 0..4200 {
        drop(
            pools
                .acquire(
                    RequestProxySource::Mihomo,
                    gateway_core::account::ResponsesUpstream::Excel,
                    &format!("anonymous-{n}"),
                    true,
                )
                .unwrap(),
        );
    }
    assert!(lock(&pools.managed).loads().is_empty());
    assert_eq!(
        pools
            .acquire(
                RequestProxySource::ProxyPool,
                gateway_core::account::ResponsesUpstream::Excel,
                "anonymous-1",
                true
            )
            .err(),
        Some(SessionProxyError::Unavailable)
    );
}

#[test]
fn expired_health_cannot_be_used_until_background_revalidation() {
    let pools = ExitPools::default();
    populate(&pools, false);
    for exit in lock(&pools.managed).exits.values_mut() {
        exit.verified_until = Some(Instant::now());
    }
    assert_eq!(
        pools
            .acquire(
                RequestProxySource::Mihomo,
                gateway_core::account::ResponsesUpstream::Excel,
                "session",
                false
            )
            .err(),
        Some(SessionProxyError::Warming)
    );
    qualify(&mut lock(&pools.managed), "a", Ok(()), Instant::now());
    assert!(
        pools
            .acquire(
                RequestProxySource::Mihomo,
                gateway_core::account::ResponsesUpstream::Excel,
                "session",
                false
            )
            .is_ok()
    );
}

#[test]
fn dynamic_window_and_deleted_node_reject_late_feedback() {
    let pools = ExitPools::default();
    populate(&pools, true);
    let lease = pools
        .acquire(
            RequestProxySource::Mihomo,
            gateway_core::account::ResponsesUpstream::Excel,
            "session",
            false,
        )
        .unwrap();
    let id = lease.node_id().to_owned();
    let mut state = lock(&pools.managed);
    let before = state.exits[&id].generation;
    let future = Instant::now() + Duration::from_secs(1201);
    state.status(future);
    if state.exits[&id].dynamic {
        assert!(state.exits[&id].generation > before);
    }
    state.replace(Vec::new(), true, future);
    state.replace(vec![(id.clone(), proxy(19000), true, false)], true, future);
    assert!(state.exits[&id].generation > before);
    drop(state);
    lease.report(SessionProxyOutcome::StreamFailure);
    assert_eq!(lock(&pools.managed).exits[&id].failures, 0);
}

#[test]
fn stream_cooldown_survives_successful_short_probe_and_duplicate_report() {
    let pools = ExitPools::default();
    populate(&pools, false);
    let lease = pools
        .acquire(
            RequestProxySource::Mihomo,
            gateway_core::account::ResponsesUpstream::Excel,
            "session",
            false,
        )
        .unwrap();
    let id = lease.node_id().to_owned();
    lease.report(SessionProxyOutcome::StreamFailure);
    let deadline = lock(&pools.managed).exits[&id].retry_after.unwrap();
    assert!(deadline > Instant::now() + Duration::from_secs(295));
    lease.report(SessionProxyOutcome::Completed);
    let mut state = lock(&pools.managed);
    qualify(&mut state, &id, Ok(()), Instant::now());
    assert_eq!(state.exits[&id].retry_after, Some(deadline));
    assert!(!state.exits[&id].ready(Instant::now()));
}

#[test]
fn stale_probe_cannot_undo_failure_or_release_a_new_probe() {
    let pools = ExitPools::default();
    populate(&pools, false);
    let lease = pools
        .acquire(
            RequestProxySource::Mihomo,
            gateway_core::account::ResponsesUpstream::Excel,
            "session",
            false,
        )
        .unwrap();
    let id = lease.node_id().to_owned();
    let (generation, revision) = {
        let state = lock(&pools.managed);
        (state.exits[&id].generation, state.exits[&id].revision)
    };
    lease.report(SessionProxyOutcome::NetworkFailure);
    let mut state = lock(&pools.managed);
    let h = state.exits.get_mut(&id).unwrap();
    let current = (h.generation, h.revision);
    h.probing = true;
    h.probe_owner = Some(current);
    state.qualify(&id, generation, revision, Ok(()), &[true], Instant::now());
    assert!(state.exits[&id].verified_until.is_none());
    assert_eq!(state.exits[&id].probe_owner, Some(current));
    assert!(state.exits[&id].probing);
}

#[test]
fn upstream_server_error_changes_score_without_quarantining_node() {
    let pools = ExitPools::default();
    populate(&pools, false);
    let lease = pools
        .acquire(
            RequestProxySource::Mihomo,
            gateway_core::account::ResponsesUpstream::Excel,
            "session",
            false,
        )
        .unwrap();
    lease.report(SessionProxyOutcome::UpstreamFailure);
    let state = lock(&pools.managed);
    assert!(state.exits[lease.node_id()].ready(Instant::now()));
    assert_eq!(state.exits[lease.node_id()].failures, 0);
}

#[test]
fn unsent_retry_rebinds_once_without_double_releasing_new_session() {
    let pools = ExitPools::default();
    populate(&pools, false);
    let first = pools
        .acquire(
            RequestProxySource::Mihomo,
            gateway_core::account::ResponsesUpstream::Excel,
            "session",
            false,
        )
        .unwrap();
    assert!(first.retry_unsent().is_none());
    first.report(SessionProxyOutcome::NetworkFailure);
    let second = first.retry_unsent().unwrap();
    assert_ne!(first.node_id(), second.node_id());
    assert!(first.retry_unsent().is_none());
    let node = second.node_id().to_owned();
    drop(first);
    assert_eq!(lock(&pools.managed).loads()[&node].0, 1);
    drop(second);
    assert_eq!(lock(&pools.managed).loads()[&node].0, 0);
}

#[test]
fn unsent_retry_never_moves_a_concurrently_active_session() {
    let pools = ExitPools::default();
    populate(&pools, false);
    let first = pools
        .acquire(
            RequestProxySource::Mihomo,
            gateway_core::account::ResponsesUpstream::Excel,
            "session",
            false,
        )
        .unwrap();
    let concurrent = pools
        .acquire(
            RequestProxySource::Mihomo,
            gateway_core::account::ResponsesUpstream::Excel,
            "session",
            false,
        )
        .unwrap();
    first.report(SessionProxyOutcome::NetworkFailure);
    assert!(first.retry_unsent().is_none());
    assert_eq!(lock(&pools.managed).loads()[concurrent.node_id()].0, 2);
}

#[test]
fn source_edits_are_staged_and_commands_redact_secrets() {
    let command: MihomoCommand = serde_json::from_value(json!({"action":"subscription_add", "subscriptions":["https://provider.example/sub?token=private-secret"]})).unwrap();
    assert!(!format!("{command:?}").contains("private-secret"));
    let old = Saved::default();
    let mut staged = old.clone();
    sources::prepare(&mut staged, &command).unwrap();
    assert!(old.subscriptions.is_empty());
    assert_eq!(staged.subscriptions.len(), 1);
    let bad: MihomoCommand = serde_json::from_value(
        json!({"action":"subscription_update","target":"missing", "subscriptions":["https://bad"]}),
    )
    .unwrap();
    assert!(sources::prepare(&mut staged, &bad).is_err());
    assert_eq!(staged.subscriptions[0].url, command.subscriptions[0]);
}

#[test]
fn private_files_are_atomic_and_do_not_leave_temporary_files() {
    let dir = tempfile::tempdir().unwrap();
    files::prepare_dir(dir.path()).unwrap();
    let file = dir.path().join("settings.json");
    files::atomic_write(&file, b"old", false).unwrap();
    files::atomic_write(&file, b"new", false).unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"new");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
