use gateway_admin::{model::proxies::ProxyLocationDetection, ports::proxy::ProxyProbe};
use gateway_core::account::OutboundProxy;
use gateway_host::proxy_probe::HttpProxyProbe;
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{any, header, path, query_param},
};

#[tokio::test]
async fn proxy_probe_supports_both_proxy_and_exit_address_families() {
    for (listen_address, exit_ip) in [
        ("127.0.0.1:0", "203.0.113.8"),
        ("127.0.0.1:0", "2001:db8::8"),
        ("[::1]:0", "203.0.113.8"),
        ("[::1]:0", "2001:db8::8"),
    ] {
        let listener = std::net::TcpListener::bind(listen_address).unwrap();
        let proxy_server = MockServer::builder().listener(listener).start().await;
        Mock::given(header("proxy-authorization", "Basic dXNlcjpwYXNzd29yZA=="))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ip": exit_ip})))
            .expect(1)
            .mount(&proxy_server)
            .await;
        let proxy =
            OutboundProxy::parse(&format!("http://user:password@{}", proxy_server.address()))
                .unwrap();
        let result = HttpProxyProbe::new("http://unresolvable.invalid/ip")
            .test(&proxy, false)
            .await;
        assert!(
            result.success,
            "{listen_address} -> {exit_ip}: {}",
            result.message
        );
        assert_eq!(result.exit_ip.unwrap().to_string(), exit_ip);
    }
}

#[tokio::test]
async fn proxy_probe_rejects_auth_errors_redirects_and_invalid_or_oversized_responses() {
    for response in [
        ResponseTemplate::new(407),
        ResponseTemplate::new(302).insert_header("Location", "http://127.0.0.1/"),
        ResponseTemplate::new(200).set_body_json(json!({"ip": "not-an-ip"})),
        ResponseTemplate::new(200).set_body_string("a".repeat(1025)),
    ] {
        let proxy_server = MockServer::start().await;
        Mock::given(any())
            .respond_with(response)
            .expect(1)
            .mount(&proxy_server)
            .await;
        let result = HttpProxyProbe::new("http://unresolvable.invalid/ip")
            .test(&OutboundProxy::parse(&proxy_server.uri()).unwrap(), false)
            .await;
        assert!(!result.success);
        assert!(result.exit_ip.is_none());
    }
}

#[tokio::test]
async fn unavailable_proxy_never_falls_back_to_direct_connection() {
    let target = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ip":"203.0.113.8"})))
        .expect(0)
        .mount(&target)
        .await;
    let unused = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = OutboundProxy::parse(&format!("http://{}", unused.local_addr().unwrap())).unwrap();
    drop(unused);
    let result = HttpProxyProbe::new(target.uri()).test(&proxy, false).await;
    assert!(!result.success);
}

#[tokio::test]
async fn invalid_certificate_configuration_should_not_fall_back_or_expose_details() {
    let target = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&target)
        .await;
    let result = HttpProxyProbe::new(target.uri())
        .with_client_builder(|_| Err("private-certificate-path"))
        .test(&OutboundProxy::parse(&target.uri()).unwrap(), false)
        .await;
    assert!(!result.success);
    assert!(!result.message.contains("private-certificate-path"));
}

#[tokio::test]
async fn dual_stack_proxy_probe_reports_both_addresses_when_available() {
    let proxy_server = MockServer::start().await;
    Mock::given(path("/v4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ip": "203.0.113.8"})))
        .expect(1)
        .mount(&proxy_server)
        .await;
    Mock::given(path("/v6"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ip": "2001:db8::8"})))
        .expect(1)
        .mount(&proxy_server)
        .await;

    let proxy = OutboundProxy::parse(&proxy_server.uri()).unwrap();
    let result = HttpProxyProbe::new_dual(
        format!("{}/v4", proxy_server.uri()),
        format!("{}/v6", proxy_server.uri()),
    )
    .test(&proxy, false)
    .await;

    assert!(result.success);
    assert_eq!(result.exit_ipv4.unwrap().to_string(), "203.0.113.8");
    assert_eq!(result.exit_ipv6.unwrap().to_string(), "2001:db8::8");
    assert!(result.message.contains("双栈可用"));
}

#[tokio::test]
async fn dual_stack_proxy_probe_reports_single_stack_when_only_one_succeeds() {
    let proxy_server = MockServer::start().await;
    Mock::given(path("/v4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ip": "203.0.113.8"})))
        .expect(1)
        .mount(&proxy_server)
        .await;
    Mock::given(path("/v6"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&proxy_server)
        .await;

    let proxy = OutboundProxy::parse(&proxy_server.uri()).unwrap();
    let result = HttpProxyProbe::new_dual(
        format!("{}/v4", proxy_server.uri()),
        format!("{}/v6", proxy_server.uri()),
    )
    .test(&proxy, false)
    .await;

    assert!(result.success);
    assert_eq!(result.exit_ipv4.unwrap().to_string(), "203.0.113.8");
    assert!(result.exit_ipv6.is_none());
    assert!(result.message.contains("仅 IPv4"));
}

fn geo(ip: &str, timezone: &str) -> serde_json::Value {
    json!({
        "success": true,
        "ip": ip,
        "country_code": "JP",
        "region": "Tokyo",
        "city": "Tokyo",
        "timezone": {"id": timezone}
    })
}

#[tokio::test]
async fn automatic_location_is_opt_in_and_queries_the_measured_ip_through_the_same_proxy() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    for enabled in [false, true] {
        let proxy_server = MockServer::start().await;
        Mock::given(path("/exit"))
            .and(header("proxy-authorization", "Basic dXNlcjpwYXNzd29yZA=="))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ip":"203.0.113.8"})))
            .expect(1)
            .mount(&proxy_server)
            .await;
        Mock::given(path("/geo/203.0.113.8"))
            .and(query_param(
                "fields",
                "ip,success,country_code,region,city,timezone.id",
            ))
            .and(header("proxy-authorization", "Basic dXNlcjpwYXNzd29yZA=="))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(geo("203.0.113.8", "Asia/Tokyo")),
            )
            .expect(u64::from(enabled))
            .mount(&proxy_server)
            .await;
        let builds = Arc::new(AtomicUsize::new(0));
        let builds_for_client = builds.clone();
        let probe = HttpProxyProbe::new("http://unresolvable.invalid/exit")
            .with_location_endpoint("http://location.invalid/geo")
            .with_client_builder(move |builder| {
                builds_for_client.fetch_add(1, Ordering::SeqCst);
                builder.build()
            });
        let proxy =
            OutboundProxy::parse(&format!("http://user:password@{}", proxy_server.address()))
                .unwrap();
        let result = probe.test(&proxy, enabled).await;
        assert!(result.success);
        assert_eq!(builds.load(Ordering::SeqCst), 1 + usize::from(enabled));
        if enabled {
            let ProxyLocationDetection::Detected { location } = result.location else {
                panic!("valid location must be detected");
            };
            assert_eq!(location.country, "JP");
            assert_eq!(location.timezone.to_string(), "Asia/Tokyo");
        } else {
            assert_eq!(result.location, ProxyLocationDetection::NotRequested);
        }
    }
}

#[tokio::test]
async fn dual_stack_location_requires_success_for_both_and_a_common_timezone() {
    for (v6_response, expected) in [
        (
            ResponseTemplate::new(200).set_body_json(geo("2001:db8::8", "Asia/Tokyo")),
            "detected",
        ),
        (
            ResponseTemplate::new(200).set_body_json(geo("2001:db8::8", "America/Los_Angeles")),
            "conflict",
        ),
        (ResponseTemplate::new(429), "failed"),
    ] {
        let proxy_server = MockServer::start().await;
        for (endpoint, ip) in [("/v4", "203.0.113.8"), ("/v6", "2001:db8::8")] {
            Mock::given(path(endpoint))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ip":ip})))
                .expect(1)
                .mount(&proxy_server)
                .await;
        }
        Mock::given(path("/geo/203.0.113.8"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(geo("203.0.113.8", "Asia/Tokyo")),
            )
            .expect(1)
            .mount(&proxy_server)
            .await;
        Mock::given(path("/geo/2001:db8::8"))
            .respond_with(v6_response)
            .expect(1)
            .mount(&proxy_server)
            .await;
        let result = HttpProxyProbe::new_dual("http://exit.invalid/v4", "http://exit.invalid/v6")
            .with_location_endpoint("http://location.invalid/geo")
            .test(&OutboundProxy::parse(&proxy_server.uri()).unwrap(), true)
            .await;
        assert!(
            result.success,
            "geo failure must not erase connectivity success"
        );
        assert_eq!(
            serde_json::to_value(result.location).unwrap()["status"],
            expected
        );
    }
}

#[tokio::test]
async fn automatic_location_rejects_incomplete_untrusted_and_oversized_results() {
    let mut incomplete = geo("203.0.113.8", "Asia/Tokyo");
    incomplete["city"] = json!("");
    for response in [
        ResponseTemplate::new(429),
        ResponseTemplate::new(302).insert_header("Location", "http://127.0.0.1/"),
        ResponseTemplate::new(200).set_body_json(geo("203.0.113.9", "Asia/Tokyo")),
        ResponseTemplate::new(200).set_body_json(geo("203.0.113.8", "Invalid/Zone")),
        ResponseTemplate::new(200).set_body_json(incomplete),
        ResponseTemplate::new(200)
            .set_body_json(json!({"success":false,"message":"untrusted secret"})),
        ResponseTemplate::new(200).set_body_string("x".repeat(8193)),
    ] {
        let proxy_server = MockServer::start().await;
        Mock::given(path("/exit"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ip":"203.0.113.8"})))
            .expect(1)
            .mount(&proxy_server)
            .await;
        Mock::given(path("/geo/203.0.113.8"))
            .respond_with(response)
            .expect(1)
            .mount(&proxy_server)
            .await;
        let result = HttpProxyProbe::new("http://exit.invalid/exit")
            .with_location_endpoint("http://location.invalid/geo")
            .test(&OutboundProxy::parse(&proxy_server.uri()).unwrap(), true)
            .await;
        assert!(result.success);
        let ProxyLocationDetection::Failed { message } = result.location else {
            panic!("invalid location must fail");
        };
        assert!(!message.contains("untrusted secret"));
    }
}

#[tokio::test]
async fn geo_lookup_never_connects_directly_when_the_proxy_rejects_it() {
    let location_server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(geo("203.0.113.8", "Asia/Tokyo")))
        .expect(0)
        .mount(&location_server)
        .await;
    let proxy_server = MockServer::start().await;
    Mock::given(path("/exit"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ip":"203.0.113.8"})))
        .expect(1)
        .mount(&proxy_server)
        .await;
    Mock::given(path("/203.0.113.8"))
        .respond_with(ResponseTemplate::new(407))
        .expect(1)
        .mount(&proxy_server)
        .await;
    let result = HttpProxyProbe::new("http://exit.invalid/exit")
        .with_location_endpoint(location_server.uri())
        .test(&OutboundProxy::parse(&proxy_server.uri()).unwrap(), true)
        .await;
    assert!(result.success);
    assert!(matches!(
        result.location,
        ProxyLocationDetection::Failed { .. }
    ));
}

#[tokio::test]
async fn automatic_location_timeout_preserves_connectivity_without_retry() {
    use std::time::Duration;

    let proxy_server = MockServer::start().await;
    Mock::given(path("/exit"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ip":"203.0.113.8"})))
        .expect(1)
        .mount(&proxy_server)
        .await;
    Mock::given(path("/geo/203.0.113.8"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(geo("203.0.113.8", "Asia/Tokyo"))
                .set_delay(Duration::from_secs(30)),
        )
        .expect(1)
        .mount(&proxy_server)
        .await;
    let probe = HttpProxyProbe::new("http://exit.invalid/exit")
        .with_location_endpoint("http://location.invalid/geo");
    let proxy = OutboundProxy::parse(&proxy_server.uri()).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), probe.test(&proxy, true))
        .await
        .expect("location query must have its own bounded timeout");
    assert!(result.success);
    assert_eq!(result.exit_ipv4, Some("203.0.113.8".parse().unwrap()));
    assert!(matches!(
        result.location,
        ProxyLocationDetection::Failed { .. }
    ));
    proxy_server.verify().await;
}
