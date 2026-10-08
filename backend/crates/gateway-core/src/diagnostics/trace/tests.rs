use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tracing::{
    Event, Metadata, Subscriber,
    span::{Attributes, Id, Record},
};

struct LogCount(Arc<AtomicUsize>);

impl Subscriber for LogCount {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, _: &Event<'_>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

#[test]
fn quality_audit_preserves_facts_without_trace_or_wire_logs() {
    const CHILD: &str = "GATEWAY_CORE_QUALITY_TRACE_TEST";
    if std::env::var_os(CHILD).is_none() {
        let thread = std::thread::current();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", thread.name().unwrap()])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "isolated quality trace test failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let logs = Arc::new(AtomicUsize::new(0));
    tracing::subscriber::with_default(LogCount(logs.clone()), || {
        {
            let ordinary = TraceContext::new("req_ordinary_trace");
            ordinary.capture("client.request.body", b"{}");
            assert_eq!(ordinary.snapshot().unwrap()["wireDumpEnabled"], true);
        }
        assert!(
            logs.swap(0, Ordering::SeqCst) > 0,
            "positive logging control"
        );
        {
            let quality = TraceContext::audit_only("req_quality_trace");
            let exchange = quality.attempt(1).exchange("http_sse");
            exchange.capture("client.request.body", br#"{"input":"synthetic prompt"}"#);
            exchange.headers(
                "upstream.response.headers",
                json!({"status":429}),
                [("x-request-id", b"fixture-upstream-id".as_slice())],
            );
            exchange.record_provider_failure(&crate::error::ProviderError::new(
                crate::error::ProviderErrorKind::QuotaExhausted,
                crate::upstream::UpstreamSendState::Sent,
            ));
            quality.record("request.finished", json!({"outcome":"Failed"}));
            let snapshot = quality.snapshot().unwrap();
            assert_eq!(snapshot["wireDumpEnabled"], false);
            assert_eq!(snapshot["wireFrames"], 0);
            assert!(
                snapshot["events"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|event| event["stage"] == "attempt.failed")
            );
            assert!(snapshot["totalEvents"].as_u64().unwrap() >= 4);
        }
        assert_eq!(logs.load(Ordering::SeqCst), 0, "including clone/drop logs");
        {
            let quality = TraceContext::audit_only("req_quality_dependency");
            quality.record_provider_failure(&crate::error::ProviderError::new(
                crate::error::ProviderErrorKind::ProviderInfrastructureUnavailable,
                crate::upstream::UpstreamSendState::NotSent,
            ));
        }
        assert_eq!(
            logs.load(Ordering::SeqCst),
            1,
            "real probe dependency failure"
        );
        tracing::warn!("fixture infrastructure failure remains visible");
        assert_eq!(logs.load(Ordering::SeqCst), 2);
    });
}
