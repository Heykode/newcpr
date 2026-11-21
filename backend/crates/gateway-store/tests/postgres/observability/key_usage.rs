use super::*;

#[tokio::test]
async fn key_statistics_preserve_local_days_missing_cost_and_key_scope_through_rollup() {
    use admin_observability::{KeyUsageMetric, TimeRange};
    use gateway_core::policy::ClientApiKeyId;
    let Some(db) = TestDatabase::create("key_statistics").await else {
        return;
    };
    let start = chrono::DateTime::parse_from_rfc3339("2026-10-01T03:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    for (id, key, model, offset, cost, tokens) in [
        (
            "known",
            "key_stats",
            Some("model_a"),
            0,
            Some("1.25"),
            10_i64,
        ),
        ("unknown", "key_stats", None, 5, None, 6),
        ("other", "key_other", Some("model_a"), 0, Some("99"), 999),
    ] {
        let at = start + TimeDelta::hours(offset);
        sqlx::query("insert into model_requests(id,client_api_key_ref,config_revision,protocol,operation,endpoint,client_transport,requested_model_id,upstream_model_id,provider_kind,provider_account_ref,upstream_transport,attempt_count,upstream_send_state,outcome,client_status_code,started_at,deadline_at,completed_at,downstream_committed_at,routing_scope,total_tokens,cost_source,cost_amount,cost_currency) values($1,$2,1,'openai','responses','/v1/responses','http_sse',$3,$3,'openai','acct_stats','http_sse',1,'sent','succeeded',200,$4,$4+interval '1 minute',$4+interval '1 second',$4,'all',$5,$6,$7::text::numeric,$8)")
            .bind(id).bind(key).bind(model).bind(at).bind(tokens)
            .bind(if cost.is_some() {"calculated"} else {"unavailable"})
            .bind(cost).bind(cost.map(|_|"USD")).execute(&db.pool).await.unwrap();
    }
    drain_analytics(&db.pool).await;
    let reader =
        admin_observability_store(&db.pool).with_timezone("America/New_York".parse().unwrap());
    let id = ClientApiKeyId::new("key_stats").unwrap();
    for range in [
        TimeRange::new(
            start - TimeDelta::milliseconds(1),
            start + TimeDelta::hours(5) + TimeDelta::milliseconds(1),
        )
        .unwrap(),
        TimeRange::new(start - TimeDelta::hours(24), start + TimeDelta::hours(48)).unwrap(),
    ] {
        let overview = reader
            .client_key_statistics(range, &id, KeyUsageMetric::CostUsd)
            .await
            .unwrap();
        assert_eq!(overview.model_count, 2);
        assert_eq!(overview.totals.requests, 2);
        assert_eq!(overview.totals.tokens, Some(16));
        assert!(overview.totals.has_missing_cost);
        assert!(overview.totals.has_estimated_cost);
        let known = overview
            .series
            .iter()
            .find(|s| s.model.as_deref() == Some("model_a"))
            .unwrap();
        assert_eq!(known.points[0].date.to_string(), "2026-09-30");
        let rows = reader.client_key_models(range, &id).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].model, "");
        assert!(rows[0].totals.cost_usd.is_none());
        assert_eq!(
            rows[1]
                .totals
                .cost_usd
                .as_deref()
                .unwrap()
                .parse::<f64>()
                .unwrap(),
            1.25
        );
        assert_eq!((rows[1].active_days, rows[1].peak_requests), (1, 1));
    }
    db.close().await;
}
