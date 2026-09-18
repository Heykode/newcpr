mod handlers;
mod import_tasks;
mod presenter;

mod personal_info {
    use chrono::{TimeZone as _, Utc};
    use gateway_admin::model::{
        AdminError,
        provider_credentials::{AccountPersonalInfo, ProviderSubscription},
    };
    use gateway_api::admin::accounts::{AccountPersonalInfoData, AccountSubscriptionData};
    use serde_json::json;

    #[test]
    fn subscription_response_exposes_only_display_fields_and_preserves_unknowns() {
        let observed = Utc.with_ymd_and_hms(2026, 9, 14, 0, 0, 0).unwrap();
        let response = AccountSubscriptionData::from(ProviderSubscription {
            starts_at: None,
            expires_at: observed,
            will_renew: None,
            billing_period: None,
            billing_currency: Some("USD".to_owned()),
            observed_at: observed,
        });
        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({
                "startsAt": null, "expiresAt": "2026-09-14T00:00:00+00:00",
                "willRenew": null, "billingPeriod": null, "billingCurrency": "USD",
                "observedAt": "2026-09-14T00:00:00+00:00"
            })
        );
    }

    #[test]
    fn personal_info_response_preserves_subscription_when_profile_fails() {
        let observed = Utc.with_ymd_and_hms(2026, 9, 14, 0, 0, 0).unwrap();
        let response = AccountPersonalInfoData::from(AccountPersonalInfo {
            profile: Err(AdminError::bad_gateway("上游服务请求失败")),
            subscription: Some(ProviderSubscription {
                starts_at: None,
                expires_at: observed,
                will_renew: None,
                billing_period: None,
                billing_currency: None,
                observed_at: observed,
            }),
        });
        let value = serde_json::to_value(response).unwrap();
        assert!(value["profile"].is_null());
        assert_eq!(value["profileError"], "上游服务请求失败");
        assert_eq!(
            value["subscription"]["expiresAt"],
            "2026-09-14T00:00:00+00:00"
        );
        assert_eq!(value.as_object().unwrap().len(), 3);
    }

    #[test]
    fn personal_info_response_preserves_unknown_subscription_and_profile_error() {
        let response = AccountPersonalInfoData::from(AccountPersonalInfo {
            profile: Err(AdminError::unavailable("Provider 服务暂不可用")),
            subscription: None,
        });
        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({
                "profile": null,
                "profileError": "Provider 服务暂不可用",
                "subscription": null,
            })
        );
    }
}

mod query {
    use gateway_admin::model::accounts::{AccountSortField, AccountStatus, SortDirection};
    use gateway_api::admin::accounts::ListQuery;
    use serde_json::json;

    #[test]
    fn account_query_should_parse_provider_status_and_sort_once() {
        let query: ListQuery = serde_json::from_value(json!({
            "page": 3,
            "pageSize": 20,
            "provider": "xai",
            "search": "  operator  ",
            "status": "normal",
            "planType": " pro ",
            "sortBy": "lastUsedAt",
            "sortDirection": "desc"
        }))
        .expect("deserialize account query");
        let query = query.validate().expect("validate account query");
        assert_eq!(query.page, 3);
        assert_eq!(query.page_size.get(), 20);
        assert!(matches!(
            query.provider_kind,
            Some(ref provider) if provider.as_str() == "xai"
        ));
        assert_eq!(query.search.as_deref(), Some("operator"));
        assert_eq!(query.status, Some(AccountStatus::Normal));
        assert_eq!(query.plan_type.as_deref(), Some("pro"));
        assert_eq!(
            query.sort.expect("sort").field,
            AccountSortField::LastUsedAt
        );
        assert_eq!(
            query.sort.expect("copy sort").direction,
            SortDirection::Desc
        );
    }

    #[test]
    fn account_query_should_parse_rate_limited_status() {
        let query: ListQuery = serde_json::from_value(json!({
            "status": "rate_limited"
        }))
        .expect("deserialize account query");

        assert_eq!(
            query.validate().expect("validate account query").status,
            Some(AccountStatus::RateLimited)
        );
    }

    #[test]
    fn account_query_should_parse_creation_and_relogin_sort_fields_from_url() {
        for (field, expected_field) in [
            ("createdAt", AccountSortField::CreatedAt),
            ("addedAt", AccountSortField::CreatedAt),
            ("reloginCount", AccountSortField::ReloginCount),
        ] {
            for (direction, expected) in
                [("asc", SortDirection::Asc), ("desc", SortDirection::Desc)]
            {
                let uri = format!("/accounts?sortBy={field}&sortDirection={direction}")
                    .parse()
                    .expect("account list URI");
                let query = axum::extract::Query::<ListQuery>::try_from_uri(&uri)
                    .expect("decode URL query")
                    .0
                    .validate()
                    .expect("validate account query");
                let sort = query.sort.expect("sort");
                assert_eq!(sort.field, expected_field);
                assert_eq!(sort.direction, expected);
            }
        }
    }

    #[test]
    fn account_query_should_reject_unbounded_page_size() {
        let query: ListQuery =
            serde_json::from_value(json!({"pageSize": 201})).expect("deserialize account query");
        assert_eq!(
            query.validate().expect_err("reject page size").field(),
            "pageSize"
        );
    }

    #[test]
    fn account_query_should_reject_incomplete_sort() {
        let query: ListQuery =
            serde_json::from_value(json!({"sortBy": "usage"})).expect("deserialize account query");
        assert_eq!(query.validate().expect_err("reject sort").field(), "sort");
    }

    #[test]
    fn account_query_should_reject_unknown_fields() {
        assert!(serde_json::from_value::<ListQuery>(json!({"id": "cred_1"})).is_err());
    }
}

mod profile_statistics {
    use chrono::NaiveDate;
    use gateway_admin::model::provider_credentials::{
        ProviderProfileActivityInsights, ProviderProfileDailyUsage, ProviderProfileInvocation,
        ProviderProfileStatistics, ProviderProfileStatisticsSummary,
    };
    use gateway_api::admin::accounts::{AccountProfileAvatarQuery, AccountProfileStatisticsData};
    use serde_json::json;

    #[test]
    fn profile_statistics_response_preserves_nullable_official_fields() {
        let response = AccountProfileStatisticsData::from(ProviderProfileStatistics {
            display_name: Some("Ada".to_owned()),
            username: Some("ada".to_owned()),
            image_url: Some("https://example.test/avatar.png".to_owned()),
            has_stats_error: false,
            summary: ProviderProfileStatisticsSummary {
                total_text_tokens: Some(409_500_000),
                peak_tokens: Some(267_000_000),
                longest_task_duration_ms: Some(51_900_000),
                current_streak_days: Some(16),
                longest_streak_days: Some(32),
            },
            daily_usage: Some(vec![ProviderProfileDailyUsage {
                date: NaiveDate::from_ymd_opt(2026, 8, 25).expect("usage date"),
                tokens: 42,
            }]),
            activity_insights: ProviderProfileActivityInsights {
                fast_mode_percent: Some(0.0),
                reasoning_effort: Some("high".to_owned()),
                reasoning_effort_percent: Some(48.0),
                skills_explored: Some(8),
                total_skills_used: Some(61),
                total_threads: Some(2_391),
                invocations: Some(vec![ProviderProfileInvocation {
                    invocation_type: "plugin".to_owned(),
                    plugin_id: Some("plugin_1".to_owned()),
                    plugin_name: Some("example".to_owned()),
                    skill_id: None,
                    skill_name: None,
                    usage_count: Some(29),
                }]),
            },
        });
        let value = serde_json::to_value(response).expect("serialize profile statistics");

        assert_eq!(value["displayName"], "Ada");
        assert_eq!(value["summary"]["totalTextTokens"], 409_500_000);
        assert_eq!(
            value["dailyUsage"][0],
            json!({"date": "2026-08-25", "tokens": 42})
        );
        assert_eq!(
            value["activityInsights"]["invocations"][0]["type"],
            "plugin"
        );
        assert_eq!(
            value["activityInsights"]["invocations"][0]["usageCount"],
            29
        );
        assert!(value.get("cycle").is_none());
        assert!(value.get("models").is_none());
        assert!(value.get("estimatedCost").is_none());
    }

    #[test]
    fn profile_avatar_query_accepts_only_bounded_cache_versions() {
        let valid: AccountProfileAvatarQuery = serde_json::from_value(json!({
            "accountId": "acct_openai",
            "version": "k9m2z1"
        }))
        .expect("decode avatar query");
        valid.validate().expect("validate avatar query");

        for version in ["", "contains-dash", "x23456789012345678901234567890123"] {
            let query: AccountProfileAvatarQuery = serde_json::from_value(json!({
                "accountId": "acct_openai",
                "version": version
            }))
            .expect("decode invalid avatar query");
            assert_eq!(
                query.validate().expect_err("reject version").field(),
                "version"
            );
        }
        assert!(
            serde_json::from_value::<AccountProfileAvatarQuery>(json!({
                "accountId": "acct_openai",
                "sourceUrl": "https://example.test/avatar"
            }))
            .is_err()
        );
    }
}

mod profile_avatar {
    use axum::body::to_bytes;
    use bytes::Bytes;
    use futures::stream;
    use gateway_admin::model::provider_credentials::ProviderProfileAvatar;
    use gateway_api::admin::accounts::profile_avatar_response;

    #[tokio::test]
    async fn profile_avatar_response_preserves_stream_metadata_and_private_cache() {
        let response = profile_avatar_response(ProviderProfileAvatar {
            content_type: Some("image/svg+xml".to_owned()),
            content_length: Some(6),
            etag: Some("\"v1\"".to_owned()),
            body: Box::pin(stream::iter([Ok(Bytes::from_static(b"avatar"))])),
        });

        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "image/svg+xml");
        assert_eq!(response.headers()["content-length"], "6");
        assert_eq!(response.headers()["etag"], "\"v1\"");
        assert_eq!(response.headers()["cache-control"], "private, max-age=3600");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert_eq!(
            response.headers()["cross-origin-resource-policy"],
            "same-origin"
        );
        assert_eq!(
            response.headers()["content-security-policy"],
            "sandbox; default-src 'none'"
        );
        assert_eq!(
            to_bytes(response.into_body(), 16)
                .await
                .expect("avatar response body"),
            Bytes::from_static(b"avatar")
        );
    }
}

mod batch_update {
    use gateway_api::admin::accounts::{BatchUpdateAccountsRequest, UpdateAccountRequest};
    use serde_json::json;

    #[test]
    fn batch_update_should_accept_complete_atomic_payload() {
        let request: BatchUpdateAccountsRequest = serde_json::from_value(json!({
            "accountIds": ["acct_openai", "acct_xai"],
            "enabled": false,
            "concurrencyLimit": null,
            "weight": 1,
            "groupIds": ["grp_00000000000000000000000000000001"]
        }))
        .expect("deserialize batch update");

        request.validate().expect("validate batch update");
    }

    #[test]
    fn batch_update_should_reject_duplicate_or_invalid_account_ids() {
        for account_ids in [
            json!(["acct_same", "acct_same"]),
            json!(["not-an-account"]),
            json!([]),
        ] {
            let request: BatchUpdateAccountsRequest = serde_json::from_value(json!({
                "accountIds": account_ids,
                "enabled": true,
                "concurrencyLimit": 8,
                "weight": 100,
                "groupIds": []
            }))
            .expect("deserialize invalid batch update");
            assert_eq!(
                request.validate().expect_err("reject account IDs").field(),
                "accountIds"
            );
        }
    }

    #[test]
    fn batch_update_should_allow_omitted_fields_and_reject_unknown_fields() {
        let request = serde_json::from_value::<BatchUpdateAccountsRequest>(json!({
            "accountIds": ["acct_test"],
            "concurrencyLimit": null,
            "weight": 1,
            "groupIds": []
        }))
        .expect("deserialize partial batch update");
        request.validate().expect("validate partial batch update");
        assert!(
            serde_json::from_value::<BatchUpdateAccountsRequest>(json!({
                "accountIds": ["acct_test"],
                "enabled": true,
                "concurrencyLimit": null,
                "weight": 1,
                "groupIds": [],
                "legacy": true
            }))
            .is_err()
        );
    }

    #[test]
    fn batch_update_should_preserve_nullable_concurrency_field_presence() {
        for (fields, expected) in [
            (json!({"weight": 2}), None),
            (json!({"concurrencyLimit": null}), Some(None)),
            (json!({"concurrencyLimit": 7}), Some(Some(7))),
        ] {
            let mut payload = fields;
            payload["accountIds"] = json!(["acct_test"]);
            let request: BatchUpdateAccountsRequest =
                serde_json::from_value(payload).expect("deserialize partial update");
            request.validate().expect("validate partial update");
            assert_eq!(request.concurrency_limit, expected);
            assert!(request.enabled.is_none());
            assert!(request.group_ids.is_none());
        }
    }

    #[test]
    fn batch_update_should_accept_proxy_only_and_reject_invalid_or_ambiguous_selection() {
        for selection in ["proxy_saved", ""] {
            let request: BatchUpdateAccountsRequest = serde_json::from_value(json!({
                "accountIds": ["acct_test"],
                "outboundProxyId": selection
            }))
            .expect("deserialize proxy-only update");
            request.validate().expect("validate proxy-only update");
            assert!(request.enabled.is_none());
            assert!(request.concurrency_limit.is_none());
            assert!(request.weight.is_none());
            assert!(request.group_ids.is_none());
        }
        for fields in [
            json!({"outboundProxyId": "invalid proxy"}),
            json!({"outboundProxyId": "p".repeat(129)}),
            json!({"outboundProxyId": "proxy_saved", "outboundProxyUrl": ""}),
            json!({"outboundProxyId": "", "outboundProxyUrl": "http://127.0.0.1:8080"}),
        ] {
            let mut payload = fields;
            payload["accountIds"] = json!(["acct_test"]);
            let request: BatchUpdateAccountsRequest =
                serde_json::from_value(payload).expect("deserialize invalid proxy selection");
            assert_eq!(
                request
                    .validate()
                    .expect_err("reject proxy selection")
                    .field(),
                "outboundProxyId"
            );
        }
        let empty: BatchUpdateAccountsRequest =
            serde_json::from_value(json!({"accountIds": ["acct_test"]})).unwrap();
        assert_eq!(empty.validate().unwrap_err().field(), "fields");
    }

    #[test]
    fn batch_update_should_reject_invalid_scheduling_bounds_and_missing_fields() {
        for (concurrency_limit, weight, field) in [
            (json!(0), json!(1), "concurrencyLimit"),
            (json!(4294967296_u64), json!(1), "concurrencyLimit"),
            (json!(null), json!(0), "weight"),
            (json!(null), json!(101), "weight"),
        ] {
            let request: BatchUpdateAccountsRequest = serde_json::from_value(json!({
                "accountIds": ["acct_test"],
                "enabled": true,
                "concurrencyLimit": concurrency_limit,
                "weight": weight,
                "groupIds": []
            }))
            .expect("deserialize invalid scheduling");
            assert_eq!(
                request.validate().expect_err("reject scheduling").field(),
                field
            );
        }
        let request = serde_json::from_value::<BatchUpdateAccountsRequest>(json!({
            "accountIds": ["acct_test"],
            "enabled": true,
            "weight": 1,
            "groupIds": []
        }))
        .expect("deserialize batch update without concurrency");
        request
            .validate()
            .expect("validate batch update without concurrency");
        let request = serde_json::from_value::<BatchUpdateAccountsRequest>(json!({
            "accountIds": ["acct_test"],
            "enabled": true,
            "concurrencyLimit": null,
            "groupIds": []
        }))
        .expect("deserialize batch update that clears concurrency");
        request
            .validate()
            .expect("explicit null should clear concurrency");
    }

    #[test]
    fn account_updates_should_reject_read_only_effective_concurrency() {
        let body = json!({
            "accountId": "acct_test",
            "enabled": true,
            "concurrencyLimit": null,
            "effectiveConcurrencyLimit": 3,
            "weight": 1,
            "groupIds": []
        });
        assert!(serde_json::from_value::<UpdateAccountRequest>(body).is_err());
        let body = json!({
            "accountIds": ["acct_test"],
            "concurrencyLimit": null,
            "effectiveConcurrencyLimit": 3
        });
        assert!(serde_json::from_value::<BatchUpdateAccountsRequest>(body).is_err());
    }

    #[test]
    fn single_update_should_require_the_same_complete_scheduling_contract() {
        let request: UpdateAccountRequest = serde_json::from_value(json!({
            "accountId": "acct_test",
            "enabled": true,
            "concurrencyLimit": 4294967295_u64,
            "weight": 100,
            "groupIds": []
        }))
        .expect("deserialize single update");
        request.validate().expect("validate single update");

        assert!(
            serde_json::from_value::<UpdateAccountRequest>(json!({
                "accountId": "acct_test",
                "enabled": true,
                "weight": 1,
                "groupIds": []
            }))
            .is_err()
        );
    }
}

mod response {
    use axum::{
        body::{Body, to_bytes},
        http::{Method, Request, StatusCode, header},
    };
    use chrono::{DateTime, TimeDelta, Utc};
    use gateway_admin::model::{
        Revision,
        accounts::{
            AccountCost, AccountCumulativeCost, AccountRecord, AccountRequestBucket, AccountUsage,
        },
        provider_credentials::{
            AccountDirectoryItem, ProviderQuota, ProviderQuotaWindow, QuotaLocalUsageAttribution,
        },
    };
    use gateway_api::admin::accounts::AccountUsageView;
    use gateway_api::admin::presenter::format_decimal_currency;
    use gateway_core::{
        account::{
            AccountConcurrencyLimit, AccountStatusFacts, AccountWeight, CredentialState,
            QuotaState, resolve_account_status,
        },
        routing::ProviderKind,
    };
    use serde_json::{Value, json};
    use tower::ServiceExt as _;

    use super::super::{AdminTestFixture, AdminTestState};

    #[tokio::test]
    async fn connection_test_get_and_post_require_admin_authentication() {
        let fixture = AdminTestFixture::new().await;
        let app =
            gateway_api::admin::accounts::router::<AdminTestState>().with_state(fixture.state());
        for method in [Method::GET, Method::POST] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri("/api/admin/accounts/connection-test")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            r#"{"accountId":"acct_test","modelId":"test-model"}"#,
                        ))
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }

    #[tokio::test]
    async fn connection_test_post_validates_json_body_and_keeps_legacy_get_compatible() {
        let fixture = AdminTestFixture::new().await;
        fixture.auth.insert_session("valid-session");
        *fixture.account.lock().expect("account fixture") = Some(account_fixture());
        let app =
            gateway_api::admin::accounts::router::<AdminTestState>().with_state(fixture.state());
        let maximum = format!("{}a", "中".repeat(1365));
        for (prompt, expected) in [
            (maximum, StatusCode::NOT_FOUND),
            ("中".repeat(1366), StatusCode::BAD_REQUEST),
            (" \n\t".to_owned(), StatusCode::BAD_REQUEST),
            ("\0".to_owned(), StatusCode::BAD_REQUEST),
        ] {
            let body = json!({
                "accountId": "acct_missing",
                "modelId": "test-model",
                "interface": "responses",
                "stream": false,
                "prompt": prompt,
            });
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/admin/accounts/connection-test")
                        .header(header::CONTENT_TYPE, "application/json")
                        .header(header::COOKIE, "cpr_admin_session=valid-session")
                        .header("x-request-id", "connection-test-post")
                        .body(Body::from(body.to_string()))
                        .expect("POST request"),
                )
                .await
                .expect("POST response");
            // A valid body reaches lookup; the requested account is intentionally absent.
            assert_eq!(response.status(), expected);
        }
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/admin/accounts/connection-test?accountId=acct_missing&modelId=test-model")
                    .header(header::COOKIE, "cpr_admin_session=valid-session")
                    .header("x-request-id", "connection-test-get")
                    .body(Body::empty())
                    .expect("legacy GET"),
            )
            .await
            .expect("legacy response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn account_routes_should_serialize_persisted_relogin_count_and_last_success() {
        let fixture = AdminTestFixture::new().await;
        fixture.auth.insert_session("valid-session");
        for count in [0, 1, 2] {
            let mut account = account_fixture();
            let last = (count > 0).then(Utc::now);
            account.account.relogin_count = count;
            account.account.last_relogin_at = last;
            *fixture.account.lock().unwrap() = Some(account);
            for (method, path, is_list) in [
                (
                    Method::GET,
                    "/api/admin/accounts?sortBy=reloginCount&sortDirection=desc",
                    true,
                ),
                (
                    Method::GET,
                    "/api/admin/accounts/detail?accountId=acct_cost",
                    false,
                ),
                (Method::POST, "/api/admin/accounts/quota/refresh", false),
            ] {
                let value = account_response(&fixture, method, path, is_list).await;
                assert_eq!(value["reloginCount"], count);
                let actual: Option<chrono::DateTime<Utc>> =
                    serde_json::from_value(value["lastReloginAt"].clone()).unwrap();
                assert_eq!(
                    actual.map(|at| at.timestamp()),
                    last.map(|at| at.timestamp())
                );
            }
        }
    }

    #[tokio::test]
    async fn account_routes_should_serialize_cumulative_costs_separately_from_window_usage() {
        let fixture = AdminTestFixture::new().await;
        fixture.auth.insert_session("valid-session");
        for amount in ["100", "105"] {
            let mut account = account_fixture();
            account.cumulative_costs = vec![cumulative_cost("USD", amount)];
            *fixture.account.lock().expect("account fixture") = Some(account);
            for (method, path, is_list) in [
                (Method::GET, "/api/admin/accounts", true),
                (
                    Method::GET,
                    "/api/admin/accounts/detail?accountId=acct_cost",
                    false,
                ),
                (
                    Method::GET,
                    "/api/admin/accounts/quota?accountId=acct_cost",
                    false,
                ),
                (Method::POST, "/api/admin/accounts/quota/refresh", false),
            ] {
                let value = account_response(&fixture, method, path, is_list).await;
                assert_eq!(
                    value["cumulativeCosts"],
                    json!([{
                        "currency": "USD",
                        "estimatedAmount": amount,
                        "estimatedAmountDisplay": format!("${amount}.00")
                    }])
                );
                assert!(value.get("cumulative_costs").is_none());
                assert_eq!(value["usage"]["costs"][0]["estimatedAmount"], "5");
                assert_eq!(
                    value["usage"]["costs"][0]["estimatedAmountDisplay"],
                    "$5.00"
                );
                assert_eq!(value["usage"]["totalTokens"], 120);
                assert_eq!(
                    value["quota"]["windows"][0]["localUsage"]["totalTokens"],
                    120
                );
                assert_eq!(value["quota"]["windows"][0]["usedPercent"], 25.0);
                assert_eq!(value["usage"]["quotaWindow"]["key"], "primary");
                assert_eq!(value["usage"]["quotaWindow"]["period"], "weekly");
                assert_eq!(value["usage"]["quotaWindow"]["usedPercent"], 25.0);
                assert_eq!(value["usage"]["quotaWindow"]["estimatedUsd"], 20.0);
                assert_eq!(value["usage"]["quotaWindow"]["remainingUsd"], 15.0);
                assert_eq!(
                    value["usage"]["quotaWindow"]["resetAt"],
                    value["quota"]["windows"][0]["resetAt"]
                );
            }
        }
    }

    #[tokio::test]
    async fn account_usage_source_should_pair_costs_with_the_selected_account_wide_window() {
        let fixture = AdminTestFixture::new().await;
        fixture.auth.insert_session("valid-session");
        let base = account_fixture();
        let mut week = base.quota.windows[0].clone();
        week.key = "account-week".to_owned();
        week.used_percent = Some(1.0);
        week.local_usage = base.usage.clone();
        let mut other_week = week.clone();
        other_week.key = "other-week".to_owned();
        other_week.used_percent = Some(75.0);
        other_week.local_usage.as_mut().expect("usage").costs = vec![cost("USD", "999")];
        let mut model_week = other_week.clone();
        model_week.key = "model-week".to_owned();
        model_week.local_usage_attribution = QuotaLocalUsageAttribution::Unavailable;
        let mut short = other_week.clone();
        short.key = "short".to_owned();
        short.window_seconds = Some(5 * 60 * 60);
        let mut month = other_week.clone();
        month.key = "month".to_owned();
        month.group = "monthly".to_owned();
        month.window_seconds = Some(30 * 24 * 60 * 60);
        let mut missing_reset = week.clone();
        missing_reset.reset_at = None;
        for (windows, expected_key, expected_period, expected_cost, expected_percent) in [
            (
                vec![
                    model_week.clone(),
                    short.clone(),
                    month.clone(),
                    week,
                    other_week,
                ],
                Some("account-week"),
                Some("weekly"),
                Some("5"),
                Some(1.0),
            ),
            (
                vec![model_week.clone(), short.clone(), month],
                Some("month"),
                Some("monthly"),
                Some("999"),
                Some(75.0),
            ),
            (
                vec![model_week, short, missing_reset],
                None,
                None,
                None,
                None,
            ),
        ] {
            let mut account = base.clone();
            account.quota.windows = windows;
            *fixture.account.lock().expect("account fixture") = Some(account);
            for (path, is_list) in [
                ("/api/admin/accounts", true),
                ("/api/admin/accounts/detail?accountId=acct_cost", false),
            ] {
                let value = account_response(&fixture, Method::GET, path, is_list).await;
                assert_eq!(value["usage"]["quotaWindow"]["key"], json!(expected_key));
                assert_eq!(
                    value["usage"]["quotaWindow"]["period"],
                    json!(expected_period)
                );
                assert_eq!(
                    value["usage"]["quotaWindow"]["usedPercent"],
                    json!(expected_percent)
                );
                assert_eq!(
                    value["usage"]["costs"][0]["estimatedAmount"],
                    json!(expected_cost)
                );
                if expected_key.is_none() {
                    assert!(value["usage"]["quotaWindow"].is_null());
                }
            }
        }
    }

    #[tokio::test]
    async fn account_routes_should_preserve_each_quota_window_reset_timestamp() {
        let fixture = AdminTestFixture::new().await;
        fixture.auth.insert_session("valid-session");
        let cases = [
            (
                "short",
                "rolling",
                "5h",
                Some(5 * 60 * 60),
                Some("2026-09-13T02:34:56.123456789Z"),
                "2026-09-13 10:34:56",
            ),
            (
                "weekly",
                "weekly",
                "7d",
                Some(7 * 24 * 60 * 60),
                Some("2026-09-20T08:00:00Z"),
                "2026-09-20 16:00:00",
            ),
            (
                "monthly",
                "monthly",
                "30d",
                Some(30 * 24 * 60 * 60),
                None,
                "—",
            ),
            ("local", "local", "local", None, None, "—"),
        ];
        let mut account = account_fixture();
        let template = account.quota.windows.pop().expect("quota window");
        account.quota.windows = cases
            .iter()
            .map(
                |(key, group, label, window_seconds, reset_at, _)| ProviderQuotaWindow {
                    key: (*key).to_owned(),
                    group: (*group).to_owned(),
                    label: (*label).to_owned(),
                    window_seconds: *window_seconds,
                    reset_at: reset_at.map(|value| {
                        DateTime::parse_from_rfc3339(value)
                            .expect("reset timestamp")
                            .with_timezone(&Utc)
                    }),
                    ..template.clone()
                },
            )
            .collect();
        *fixture.account.lock().expect("account fixture") = Some(account);

        for (method, path, is_list) in [
            (Method::GET, "/api/admin/accounts", true),
            (
                Method::GET,
                "/api/admin/accounts/detail?accountId=acct_cost",
                false,
            ),
            (
                Method::GET,
                "/api/admin/accounts/quota?accountId=acct_cost",
                false,
            ),
            (Method::POST, "/api/admin/accounts/quota/refresh", false),
        ] {
            let value = account_response(&fixture, method, path, is_list).await;
            let windows = value["quota"]["windows"].as_array().expect("quota windows");
            assert_eq!(windows.len(), cases.len(), "{path}");
            for (window, (key, group, label, seconds, reset_at, display)) in
                windows.iter().zip(cases)
            {
                assert_eq!(window["key"], key, "{path}");
                assert_eq!(window["group"], group, "{path}");
                assert_eq!(window["windowLabelDisplay"], label, "{path}");
                assert_eq!(window["windowSeconds"], json!(seconds), "{path}");
                assert_eq!(
                    window.get("resetAt"),
                    Some(&json!(reset_at)),
                    "{path}: {key}"
                );
                assert_eq!(window["resetAtDisplay"], display, "{path}: {key}");
                assert_eq!(window["usedPercent"], 25.0, "{path}: {key}");
                assert_eq!(window["usedPercentDisplay"], "25.0%", "{path}: {key}");
                assert!(window.get("reset_at").is_none());
            }
        }
    }

    #[tokio::test]
    async fn account_routes_should_preserve_expired_quota_reset_timestamp_and_limit() {
        let fixture = AdminTestFixture::new().await;
        fixture.auth.insert_session("valid-session");
        let mut account = account_fixture();
        account.quota.limit_reached = true;
        let window = &mut account.quota.windows[0];
        window.reset_at = Some(
            DateTime::parse_from_rfc3339("2020-01-02T03:04:05+08:00")
                .expect("expired reset timestamp")
                .with_timezone(&Utc),
        );
        window.used_percent = Some(100.0);
        window.limit_reached = true;
        *fixture.account.lock().expect("account fixture") = Some(account);

        for (path, is_list) in [
            ("/api/admin/accounts", true),
            ("/api/admin/accounts/detail?accountId=acct_cost", false),
        ] {
            let value = account_response(&fixture, Method::GET, path, is_list).await;
            let quota = &value["quota"];
            assert_eq!(quota["limitReached"], true, "{path}");
            let window = &quota["windows"][0];
            assert_eq!(window["resetAt"], "2020-01-01T19:04:05Z", "{path}");
            assert_eq!(window["resetAtDisplay"], "2020-01-02 03:04:05", "{path}");
            assert_eq!(window["usedPercent"], 100.0, "{path}");
            assert_eq!(window["usedPercentDisplay"], "100.0%", "{path}");
            assert_eq!(window["limitReached"], true, "{path}");
        }
    }

    #[tokio::test]
    async fn account_routes_should_project_effective_concurrency_without_rewriting_overrides() {
        let fixture = AdminTestFixture::new().await;
        fixture.auth.insert_session("valid-session");
        for limit in [None, Some(1), Some(8), Some(u32::MAX)] {
            for enabled in [true, false] {
                let mut item = account_fixture();
                item.account.enabled = enabled;
                item.account.concurrency_limit =
                    limit.map(|value| AccountConcurrencyLimit::new(value).expect("override"));
                let stored = item.account.clone();
                *fixture.account.lock().expect("account fixture") = Some(item);
                assert_account_concurrency(&fixture, limit, limit.unwrap_or(3)).await;
                assert_eq!(
                    fixture
                        .account
                        .lock()
                        .expect("account fixture")
                        .as_ref()
                        .expect("account")
                        .account,
                    stored,
                );
            }
        }
    }

    #[tokio::test]
    async fn account_routes_should_follow_live_global_concurrency_changes() {
        let fixture = AdminTestFixture::new().await;
        fixture.auth.insert_session("valid-session");
        *fixture.account.lock().expect("account fixture") = Some(account_fixture());
        assert_account_concurrency(&fixture, None, 3).await;

        for default in [7, 1, u32::MAX] {
            let response = update_global_concurrency(&fixture, default).await;
            assert_eq!(response.status(), StatusCode::OK);
            for limit in [None, Some(8)] {
                fixture
                    .account
                    .lock()
                    .expect("account fixture")
                    .as_mut()
                    .expect("account")
                    .account
                    .concurrency_limit =
                    limit.map(|value| AccountConcurrencyLimit::new(value).expect("override"));
                assert_account_concurrency(&fixture, limit, limit.unwrap_or(default)).await;
                assert_eq!(
                    fixture
                        .account
                        .lock()
                        .expect("account fixture")
                        .as_ref()
                        .expect("account")
                        .account
                        .concurrency_limit
                        .map(|value| value.get()),
                    limit,
                );
            }
        }
    }

    #[tokio::test]
    async fn account_routes_should_keep_inherited_concurrency_after_rejecting_zero_default() {
        let fixture = AdminTestFixture::new().await;
        fixture.auth.insert_session("valid-session");
        *fixture.account.lock().expect("account fixture") = Some(account_fixture());
        assert_eq!(
            update_global_concurrency(&fixture, 0).await.status(),
            StatusCode::BAD_REQUEST,
        );
        assert_account_concurrency(&fixture, None, 3).await;
    }

    #[tokio::test]
    async fn account_routes_should_not_invent_concurrency_for_invalid_runtime_settings() {
        let fixture = AdminTestFixture::new().await;
        fixture.auth.insert_session("valid-session");
        *fixture.account.lock().expect("account fixture") = Some(account_fixture());
        fixture
            .settings
            .settings
            .lock()
            .expect("settings")
            .max_concurrent_per_account = 0;
        for path in [
            "/api/admin/accounts",
            "/api/admin/accounts/detail?accountId=acct_cost",
        ] {
            let response = gateway_api::admin::accounts::router::<AdminTestState>()
                .with_state(fixture.state())
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(header::COOKIE, "cpr_admin_session=valid-session")
                        .body(Body::empty())
                        .expect("account request"),
                )
                .await
                .expect("account response");
            assert_eq!(
                response.status(),
                StatusCode::INTERNAL_SERVER_ERROR,
                "{path}"
            );
        }
    }

    async fn assert_account_concurrency(
        fixture: &AdminTestFixture,
        configured: Option<u32>,
        effective: u32,
    ) {
        for (method, path, is_list) in [
            (Method::GET, "/api/admin/accounts", true),
            (
                Method::GET,
                "/api/admin/accounts/detail?accountId=acct_cost",
                false,
            ),
            (Method::POST, "/api/admin/accounts/quota/refresh", false),
        ] {
            let value = account_response(fixture, method, path, is_list).await;
            assert_eq!(value["concurrencyLimit"], json!(configured), "{path}");
            assert_eq!(value["effectiveConcurrencyLimit"], effective, "{path}");
            assert!(value.get("effective_concurrency_limit").is_none());
        }
    }

    async fn update_global_concurrency(
        fixture: &AdminTestFixture,
        limit: u32,
    ) -> axum::response::Response {
        let current = fixture
            .services
            .settings()
            .load()
            .await
            .expect("runtime settings");
        let mut body = serde_json::to_value(
            gateway_api::admin::settings::RuntimeSettingsView::from(current),
        )
        .expect("runtime settings JSON");
        body.as_object_mut()
            .expect("runtime settings object")
            .remove("updatedAt");
        body["maxConcurrentPerAccount"] = json!(limit);
        gateway_api::admin::settings::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/admin/settings/update")
                    .header("x-request-id", "req_account_concurrency")
                    .header(header::COOKIE, "cpr_admin_session=valid-session")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("settings update"),
            )
            .await
            .expect("settings response")
    }

    #[tokio::test]
    async fn account_routes_should_project_health_non_completion_counts_and_fill_empty_buckets() {
        let fixture = AdminTestFixture::new().await;
        fixture.auth.insert_session("valid-session");
        let current_bucket = Utc::now().timestamp().div_euclid(300) * 300;
        let incomplete_start = chrono::DateTime::<Utc>::from_timestamp(current_bucket - 600, 0)
            .expect("incomplete bucket time");
        let failed_start = incomplete_start - TimeDelta::minutes(5);
        let observed_buckets = vec![
            AccountRequestBucket {
                bucket_start: failed_start,
                request_count: 1,
                success_count: 0,
                error_count: 1,
                non_completion_count: 0,
            },
            AccountRequestBucket {
                bucket_start: incomplete_start,
                request_count: 5,
                success_count: 1,
                error_count: 2,
                non_completion_count: 2,
            },
        ];
        for health_timeline in [observed_buckets, Vec::new()] {
            let has_observations = !health_timeline.is_empty();
            let mut account = account_fixture();
            account.health_timeline = health_timeline;
            *fixture.account.lock().expect("account fixture") = Some(account);
            for (path, is_list) in [
                ("/api/admin/accounts", true),
                ("/api/admin/accounts/detail?accountId=acct_cost", false),
            ] {
                let value = account_response(&fixture, Method::GET, path, is_list).await;
                let buckets = value["healthTimeline"].as_array().expect("health buckets");
                assert_eq!(buckets.len(), 6);
                assert_eq!(
                    buckets
                        .iter()
                        .map(|bucket| bucket["nonCompletionCount"]
                            .as_u64()
                            .expect("non-completion count"))
                        .sum::<u64>(),
                    if has_observations { 2 } else { 0 },
                    "{path}"
                );
                for bucket in buckets {
                    let expected =
                        if has_observations && bucket["key"] == incomplete_start.to_rfc3339() {
                            (5, 1, 2, 2)
                        } else if has_observations && bucket["key"] == failed_start.to_rfc3339() {
                            (1, 0, 1, 0)
                        } else {
                            (0, 0, 0, 0)
                        };
                    assert_eq!(bucket["requestCount"], expected.0, "{path}");
                    assert_eq!(bucket["successCount"], expected.1, "{path}");
                    assert_eq!(bucket["errorCount"], expected.2, "{path}");
                    assert_eq!(bucket["nonCompletionCount"], expected.3, "{path}");
                    assert_eq!(bucket["inFlightCount"], 0);
                    assert!(bucket.get("non_completion_count").is_none());
                }
            }
        }
    }

    #[tokio::test]
    async fn account_routes_should_preserve_cumulative_cost_evidence_without_quota() {
        let fixture = AdminTestFixture::new().await;
        fixture.auth.insert_session("valid-session");
        for (costs, expected) in [
            (
                vec![
                    cumulative_cost("USD", "105.0000000001"),
                    cumulative_cost("CNY", "0.1204956"),
                ],
                json!([
                    {
                        "currency": "USD",
                        "estimatedAmount": "105.0000000001",
                        "estimatedAmountDisplay": "$105.00"
                    },
                    {
                        "currency": "CNY",
                        "estimatedAmount": "0.1204956",
                        "estimatedAmountDisplay": "CNY 0.1204956"
                    }
                ]),
            ),
            (
                vec![cumulative_cost("USD", "0")],
                json!([{
                    "currency": "USD",
                    "estimatedAmount": "0",
                    "estimatedAmountDisplay": "$0.00"
                }]),
            ),
            (
                vec![cumulative_cost("USD", "0.1204956")],
                json!([{
                    "currency": "USD",
                    "estimatedAmount": "0.1204956",
                    "estimatedAmountDisplay": "$0.1205"
                }]),
            ),
            (vec![], json!([])),
        ] {
            let mut account = account_fixture();
            account.cumulative_costs = costs;
            account.quota.windows.clear();
            *fixture.account.lock().expect("account fixture") = Some(account);
            for (path, is_list) in [
                ("/api/admin/accounts", true),
                ("/api/admin/accounts/detail?accountId=acct_cost", false),
            ] {
                let value = account_response(&fixture, Method::GET, path, is_list).await;
                assert_eq!(value["cumulativeCosts"], expected);
                assert_eq!(value["quota"]["windows"], json!([]));
                assert_eq!(value["usage"]["costs"], json!([]));
                assert!(value["usage"]["requestCount"].is_null());
                assert!(value["usage"]["totalTokens"].is_null());
            }
        }
    }

    #[tokio::test]
    async fn account_routes_should_display_large_cumulative_totals_without_zero_fallback() {
        let fixture = AdminTestFixture::new().await;
        fixture.auth.insert_session("valid-session");
        for amount in [
            "10000000000.0000000001",
            "999999999999999999999999999999.9999999999",
        ] {
            assert!(amount.parse::<gateway_core::metering::Decimal>().is_err());
            for has_quota in [true, false] {
                let mut account = account_fixture();
                account.cumulative_costs = vec![
                    cumulative_cost("USD", amount),
                    cumulative_cost("CNY", amount),
                ];
                if !has_quota {
                    account.quota.windows.clear();
                }
                *fixture.account.lock().expect("account fixture") = Some(account);
                for (path, is_list) in [
                    ("/api/admin/accounts", true),
                    ("/api/admin/accounts/detail?accountId=acct_cost", false),
                ] {
                    let value = account_response(&fixture, Method::GET, path, is_list).await;
                    assert_eq!(
                        value["cumulativeCosts"],
                        json!([
                            {
                                "currency": "USD",
                                "estimatedAmount": amount,
                                "estimatedAmountDisplay": format!("${amount}")
                            },
                            {
                                "currency": "CNY",
                                "estimatedAmount": amount,
                                "estimatedAmountDisplay": format!("CNY {amount}")
                            }
                        ])
                    );
                    if has_quota {
                        assert_eq!(value["usage"]["costs"][0]["estimatedAmount"], "5");
                        assert_eq!(
                            value["usage"]["costs"][0]["estimatedAmountDisplay"],
                            "$5.00"
                        );
                        assert_eq!(value["usage"]["totalTokens"], 120);
                    } else {
                        assert_eq!(value["usage"]["costs"], json!([]));
                        assert!(value["usage"]["totalTokens"].is_null());
                    }
                }
            }
        }
    }

    async fn account_response(
        fixture: &AdminTestFixture,
        method: Method,
        path: &str,
        is_list: bool,
    ) -> Value {
        let body = if method == Method::POST {
            Body::from(json!({"accountId": "acct_cost"}).to_string())
        } else {
            Body::empty()
        };
        let response = gateway_api::admin::accounts::router::<AdminTestState>()
            .with_state(fixture.state())
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("x-request-id", "req_account_cumulative_costs")
                    .header(header::COOKIE, "cpr_admin_session=valid-session")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(body)
                    .expect("account request"),
            )
            .await
            .expect("account response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("account response body");
        let mut value: Value = serde_json::from_slice(&body).expect("account JSON");
        if is_list {
            value["data"]["items"][0].take()
        } else {
            value["data"]["account"].take()
        }
    }

    fn cost(currency: &str, amount: &str) -> AccountCost {
        AccountCost {
            currency: currency.to_owned(),
            amount: amount.parse().expect("decimal cost"),
        }
    }

    fn cumulative_cost(currency: &str, amount: &str) -> AccountCumulativeCost {
        AccountCumulativeCost {
            currency: currency.to_owned(),
            amount: amount.parse().expect("cumulative cost amount"),
        }
    }

    fn account_fixture() -> AccountDirectoryItem {
        let now = Utc::now();
        let quota = QuotaState::allowed(now.into());
        AccountDirectoryItem {
            effective_concurrency_limit: std::num::NonZeroU32::new(3).expect("concurrency limit"),
            account: AccountRecord {
                id: "acct_cost".to_owned(),
                provider_kind: ProviderKind::new("openai").expect("provider"),
                groups: vec![],
                name: "Cost account".to_owned(),
                email: None,
                upstream_user_id: None,
                upstream_account_id: None,
                plan_type: None,
                authentication_kind: "oauth".to_owned(),
                credential_revision: Revision::new(1).expect("revision"),
                relogin_count: 0,
                last_relogin_at: None,
                has_refresh_token: true,
                access_token_expires_at: None,
                next_refresh_at: None,
                enabled: true,
                turn_state_injection_enabled: false,
                concurrency_limit: None,
                weight: AccountWeight::DEFAULT,
                outbound_proxy: None,
                credential_state: CredentialState::Ready,
                credential_observed_at: now,
                quota,
                last_error_reason: None,
                last_error_message: None,
                created_at: now,
                updated_at: now,
            },
            in_flight: None,
            health_timeline: vec![],
            cumulative_costs: vec![],
            plan_type_display: None,
            projection: resolve_account_status(
                &AccountStatusFacts {
                    enabled: true,
                    credential_state: CredentialState::Ready,
                    access_token_expires_at: None,
                    quota,
                    rate_limited_until: None,
                    last_error_reason: None,
                    last_error_message: None,
                },
                now.into(),
            ),
            usage: Some(AccountUsage {
                account_id: "acct_cost".to_owned(),
                request_count: 1,
                success_count: 1,
                input_tokens: Some(100),
                output_tokens: Some(20),
                cached_tokens: Some(0),
                cache_write_tokens: Some(0),
                reasoning_tokens: Some(0),
                image_input_tokens: Some(0),
                image_output_tokens: Some(0),
                image_request_count: 0,
                image_request_failed_count: 0,
                total_tokens: Some(120),
                cost_coverage: Default::default(),
                costs: vec![cost("USD", "5")],
                last_used_at: Some(now),
                request_buckets: vec![],
                models: vec![],
            }),
            quota: ProviderQuota {
                plan_type: None,
                observed_at: Some(now),
                refresh_token_expires_at: None,
                windows: vec![ProviderQuotaWindow {
                    key: "primary".to_owned(),
                    group: "weekly".to_owned(),
                    label: "7d".to_owned(),
                    limit_id: None,
                    limit_name: None,
                    role: None,
                    local_usage_attribution: QuotaLocalUsageAttribution::AccountWide,
                    window_seconds: Some(7 * 24 * 60 * 60),
                    used_percent: Some(25.0),
                    reset_at: Some(now + TimeDelta::hours(1)),
                    limit_reached: false,
                    local_usage: None,
                    provider_data: None,
                }],
                limit_reached: false,
                provider_data: None,
            },
        }
    }

    #[test]
    fn account_usage_view_should_keep_unobserved_numbers_null() {
        let view = AccountUsageView {
            window_label_display: "周/月额度窗口".to_owned(),
            quota_window: None,
            request_count: None,
            request_count_display: "-".to_owned(),
            input_tokens: None,
            input_tokens_display: "-".to_owned(),
            output_tokens: None,
            output_tokens_display: "-".to_owned(),
            cached_tokens: None,
            cached_tokens_display: "-".to_owned(),
            reasoning_tokens: None,
            reasoning_tokens_display: "-".to_owned(),
            image_input_tokens: None,
            image_input_tokens_display: "-".to_owned(),
            image_output_tokens: None,
            image_output_tokens_display: "-".to_owned(),
            image_request_count: None,
            image_request_count_display: "-".to_owned(),
            image_request_failed_count: None,
            image_request_failed_count_display: "-".to_owned(),
            total_tokens: None,
            total_tokens_display: "-".to_owned(),
            created_tokens: None,
            created_tokens_display: "-".to_owned(),
            read_tokens: None,
            read_tokens_display: "-".to_owned(),
            last_used_at: None,
            last_used_at_display: "-".to_owned(),
            cost_estimate_status: "unknown".to_owned(),
            known_cost_count: None,
            partial_cost_count: None,
            unknown_cost_count: None,
            costs: Vec::new(),
            models: Vec::new(),
        };
        let value = serde_json::to_value(view).expect("serialize account usage");
        assert_eq!(value["windowLabelDisplay"], "周/月额度窗口");
        assert!(value["inputTokens"].is_null());
        assert!(value["totalTokens"].is_null());
        assert!(value["reasoningTokens"].is_null());
        assert_eq!(value["reasoningTokensDisplay"], "-");
        assert_eq!(value["createdTokensDisplay"], "-");
    }

    #[test]
    fn account_usage_currency_should_limit_usd_to_four_fraction_digits() {
        assert_eq!(format_decimal_currency("0.1204956", "USD"), "$0.1205");
        assert_eq!(format_decimal_currency("0.99996", "USD"), "$1.00");
        assert_eq!(format_decimal_currency("0.0300", "USD"), "$0.03");
        assert_eq!(format_decimal_currency("12.3456", "USD"), "$12.35");
        assert_eq!(format_decimal_currency("0.1204956", "CNY"), "CNY 0.1204956");
    }
}

mod actions {
    use base64::Engine as _;
    use gateway_admin::model::{
        Revision,
        accounts::{
            AccountConnectionTestEvent as DomainConnectionTestEvent, ConnectionTestEndpoint,
        },
        provider_credentials::{
            CredentialDeletionResult, CredentialImportResult, CredentialMutationResult,
        },
    };
    use gateway_api::admin::accounts::{
        AccountActionRequest, AccountConnectionTestEvent, AccountDeletionData,
        AccountDeletionRequest, AccountExportData, AccountExportQuery, AccountIdQuery,
        AccountImportData, AccountImportRequest, AccountMutationData, AccountRefreshRequest,
        AccountResetCreditConsumeRequest, AccountTestQuery, CompleteAccountAuthorizationRequest,
        RotateAccountRequest, StartAccountAuthorizationRequest,
    };
    use gateway_core::{
        account::ProviderAccountId, engine::probe::AccountProbeErrorSource,
        error::GatewayErrorKind, upstream::UpstreamSendState,
    };
    use serde_json::json;

    #[test]
    fn export_should_require_explicit_unique_ids_and_confirmation() {
        let valid: AccountExportQuery = serde_json::from_value(json!({
            "accountIds": "acct_1,acct_2",
            "confirm": "export_sensitive_accounts"
        }))
        .expect("decode export query");
        assert_eq!(
            valid
                .into_ids()
                .expect("valid export")
                .into_iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>(),
            ["acct_1".to_owned(), "acct_2".to_owned()]
        );

        for query in [
            json!({ "accountIds": "", "confirm": "export_sensitive_accounts" }),
            json!({ "accountIds": "acct_1,acct_1", "confirm": "export_sensitive_accounts" }),
            json!({ "accountIds": "acct_1", "confirm": "yes" }),
        ] {
            assert!(
                serde_json::from_value::<AccountExportQuery>(query)
                    .expect("decode invalid export query")
                    .into_ids()
                    .is_err()
            );
        }
    }

    #[test]
    fn account_actions_should_require_frozen_account_ids_and_reject_unknown_revision() {
        let id: AccountIdQuery =
            serde_json::from_value(json!({ "accountId": "acct_1" })).expect("decode ID query");
        assert!(id.validate().is_ok());
        let action: AccountActionRequest =
            serde_json::from_value(json!({ "accountId": "legacy-id" })).expect("decode action");
        assert_eq!(action.validate().unwrap_err().field(), "accountId");
        let refresh: AccountRefreshRequest = serde_json::from_value(json!({
            "accountId": "acct_1"
        }))
        .expect("decode refresh");
        assert!(refresh.validate().is_ok());
        assert!(
            serde_json::from_value::<AccountRefreshRequest>(json!({
                "accountId": "acct_1",
                "expectedConfigRevision": 0
            }))
            .is_err()
        );
    }

    #[test]
    fn reset_credit_consume_should_require_canonical_v4_idempotency_key() {
        let valid: AccountResetCreditConsumeRequest = serde_json::from_value(json!({
            "accountId": "acct_1",
            "creditId": "credit_1",
            "redeemRequestId": "8fbf302d-11df-4bd5-82e4-08e4b3df7874"
        }))
        .expect("decode reset-credit consume");
        valid.validate().expect("validate reset-credit consume");

        for (redeem_request_id, field) in [
            ("019c0000-0000-7000-8000-000000000000", "redeemRequestId"),
            ("8FBF302D-11DF-4BD5-82E4-08E4B3DF7874", "redeemRequestId"),
            ("invalid", "redeemRequestId"),
        ] {
            let request: AccountResetCreditConsumeRequest = serde_json::from_value(json!({
                "accountId": "acct_1",
                "redeemRequestId": redeem_request_id
            }))
            .expect("decode invalid reset-credit consume");
            assert_eq!(request.validate().expect_err("reject UUID").field(), field);
        }

        let invalid_credit: AccountResetCreditConsumeRequest = serde_json::from_value(json!({
            "accountId": "acct_1",
            "creditId": " ",
            "redeemRequestId": "8fbf302d-11df-4bd5-82e4-08e4b3df7874"
        }))
        .expect("decode invalid credit");
        assert_eq!(
            invalid_credit
                .validate()
                .expect_err("reject credit")
                .field(),
            "creditId"
        );
    }

    #[test]
    fn credential_recovery_requests_should_not_accept_client_revision_fences() {
        let authorization: StartAccountAuthorizationRequest = serde_json::from_value(json!({
            "provider": "openai",
            "name": "reauthorize",
            "accountId": "acct_1"
        }))
        .expect("decode reauthorization");
        assert!(authorization.validate().is_ok());
        assert!(
            serde_json::from_value::<StartAccountAuthorizationRequest>(json!({
                "provider": "openai",
                "name": "reauthorize",
                "accountId": "acct_1",
                "expectedCredentialRevision": 1
            }))
            .is_err()
        );

        let rotation: RotateAccountRequest = serde_json::from_value(json!({
            "provider": "openai",
            "accountId": "acct_1",
            "accessToken": "header.payload.signature",
            "refreshToken": "refresh-token",
            "idToken": "id-header.id-payload.id-signature"
        }))
        .expect("decode rotation");
        assert!(rotation.validate().is_ok());
        assert!(
            serde_json::from_value::<RotateAccountRequest>(json!({
                "provider": "openai",
                "accountId": "acct_1",
                "expectedCredentialRevision": 1,
                "accessToken": "header.payload.signature"
            }))
            .is_err()
        );
    }

    #[test]
    fn credential_mutation_response_should_not_expose_internal_revision() {
        let response = AccountMutationData::from(CredentialMutationResult {
            config_revision: Revision::new(8).expect("config revision"),
            account_id: ProviderAccountId::new("acct_1").expect("account ID"),
            credential_revision: Some(Revision::new(9).expect("credential revision")),
        });

        assert_eq!(
            serde_json::to_value(response).expect("serialize credential mutation"),
            json!({ "accountId": "acct_1" })
        );
    }

    #[test]
    fn account_import_should_use_provider_and_opaque_data_fields() {
        let valid: AccountImportRequest = serde_json::from_value(json!({
            "provider": "openai",
            "data": {
                "providerOwnedUnknownField": {"nested": [1, 2, 3]},
                "accounts": [{"credentials": {"access_token": "provider-validates-this"}}]
            }
        }))
        .expect("decode account import");
        assert!(valid.validate().is_ok());

        let invalid: AccountImportRequest = serde_json::from_value(json!({
            "provider": "xai",
            "data": []
        }))
        .expect("decode invalid account import");
        assert_eq!(invalid.validate().unwrap_err().field(), "data");
        assert!(
            serde_json::from_value::<AccountImportRequest>(json!({
                "provider": "openai",
                "document": {}
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<AccountImportRequest>(json!({
                "provider": "openai",
                "expectedConfigRevision": 7,
                "data": {}
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<AccountImportRequest>(json!({
                "provider": "openai",
                "data": {},
                "groupIds": []
            }))
            .is_err()
        );
    }

    #[test]
    fn oauth_complete_should_not_accept_group_assignment() {
        let flow_id = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7_u8; 32]);
        let request: CompleteAccountAuthorizationRequest = serde_json::from_value(json!({
            "provider": "openai",
            "flowId": flow_id,
            "callbackUrl": "http://localhost/callback?code=ok"
        }))
        .expect("decode OAuth completion");
        assert!(request.validate().is_ok());
        assert!(
            serde_json::from_value::<CompleteAccountAuthorizationRequest>(json!({
                "provider": "xai",
                "flowId": "flow_test",
                "callbackUrl": "http://localhost/callback?code=ok",
                "groupIds": []
            }))
            .is_err()
        );
    }

    #[test]
    fn account_deletion_should_validate_one_provider_batch_and_emit_account_ids() {
        let request: AccountDeletionRequest = serde_json::from_value(json!({
            "provider": "xai",
            "accountIds": ["acct_1", "acct_2"]
        }))
        .expect("decode account deletion");
        assert!(request.validate().is_ok());

        let duplicate: AccountDeletionRequest = serde_json::from_value(json!({
            "provider": "xai",
            "accountIds": ["acct_1", "acct_1"]
        }))
        .expect("decode duplicate account deletion");
        assert_eq!(duplicate.validate().unwrap_err().field(), "accountIds");

        let response = AccountDeletionData::from(CredentialDeletionResult {
            config_revision: Revision::new(9).expect("revision"),
            account_ids: vec![
                ProviderAccountId::new("acct_1").expect("account ID"),
                ProviderAccountId::new("acct_2").expect("account ID"),
            ],
        });
        assert_eq!(
            serde_json::to_value(response).expect("serialize account deletion"),
            json!({
                "deletedCount": 2,
                "accountIds": ["acct_1", "acct_2"]
            })
        );
    }

    #[test]
    fn account_import_response_should_emit_account_ids() {
        let response = AccountImportData::from_result(CredentialImportResult {
            config_revision: Revision::new(8).expect("revision"),
            credential_ids: vec![ProviderAccountId::new("acct_imported").expect("account ID")],
        });
        assert_eq!(
            serde_json::to_value(response).expect("serialize account import"),
            json!({
                "importedCount": 1,
                "accountIds": ["acct_imported"]
            })
        );
    }

    #[test]
    fn connection_test_should_require_model_in_query() {
        let query: AccountTestQuery = serde_json::from_value(json!({
            "accountId": "acct_1",
            "modelId": " "
        }))
        .expect("decode connection test query");
        assert_eq!(query.validate().unwrap_err().field(), "modelId");
    }

    #[test]
    fn connection_test_query_should_default_legacy_values_and_validate_options() {
        let query: AccountTestQuery = serde_json::from_value(json!({
            "accountId": "acct_1",
            "modelId": "grok-4.5"
        }))
        .expect("decode legacy connection test query");
        assert!(query.validate().is_ok());
        assert_eq!(query.interface, "responses");
        assert_eq!(query.prompt, "Reply with exactly OK.");
        assert!(query.stream);

        for (field, value) in [
            ("interface", json!("chat")),
            ("prompt", json!(" ")),
            ("prompt", json!("\u{0000}")),
        ] {
            let query: AccountTestQuery = serde_json::from_value(json!({
                "accountId": "acct_1",
                "modelId": "grok-4.5",
                field: value
            }))
            .expect("decode invalid connection test query");
            assert_eq!(query.validate().unwrap_err().field(), field);
        }

        let multiline: AccountTestQuery = serde_json::from_value(json!({
            "accountId": "acct_1",
            "modelId": "grok-4.5",
            "prompt": "line one\nline two\twith tab"
        }))
        .expect("decode multiline connection test query");
        assert!(multiline.validate().is_ok());

        let oversized = "界".repeat(1366);
        let query: AccountTestQuery = serde_json::from_value(json!({
            "accountId": "acct_1",
            "modelId": "grok-4.5",
            "prompt": oversized
        }))
        .expect("decode oversized connection test query");
        assert_eq!(query.validate().unwrap_err().field(), "prompt");
    }

    #[test]
    fn connection_test_query_decodes_real_url_options_and_utf8_multiline_prompt() {
        let prompt = "请原样回复：OK + &=#\n第二行\r\n\t结束";
        for interface in ["responses", "completions"] {
            let encoded = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("accountId", "acct_1")
                .append_pair("modelId", "gpt-5.4")
                .append_pair("interface", interface)
                .append_pair("stream", "false")
                .append_pair("prompt", prompt)
                .finish();
            let uri = format!("/accounts/test?{encoded}")
                .parse()
                .expect("connection test URI");
            let query = axum::extract::Query::<AccountTestQuery>::try_from_uri(&uri)
                .expect("decode URL query")
                .0;
            query.validate().expect("validate decoded query");
            assert_eq!(query.account_id, "acct_1");
            assert_eq!(query.model_id, "gpt-5.4");
            assert_eq!(query.interface, interface);
            assert!(!query.stream);
            assert_eq!(query.prompt, prompt);
        }
        let uri = "/accounts/test?accountId=acct_1&modelId=gpt-5.4"
            .parse()
            .expect("legacy URI");
        let query = axum::extract::Query::<AccountTestQuery>::try_from_uri(&uri)
            .expect("decode legacy URL query")
            .0;
        assert!(query.stream);
        assert_eq!(query.interface, "responses");
        assert_eq!(query.prompt, "Reply with exactly OK.");

        let uri = "/accounts/test?accountId=acct_1&modelId=gpt-5.4&stream=not-a-bool"
            .parse()
            .expect("invalid stream URI");
        assert!(axum::extract::Query::<AccountTestQuery>::try_from_uri(&uri).is_err());
    }

    #[test]
    fn connection_test_events_should_preserve_the_existing_frontend_contract() {
        let events = [
            DomainConnectionTestEvent::Started {
                model: "grok-4.5".to_owned(),
                endpoint: ConnectionTestEndpoint::Responses,
            },
            DomainConnectionTestEvent::Request {
                model: "grok-4.5".to_owned(),
                input_text: "Reply with exactly OK.".to_owned(),
                endpoint: ConnectionTestEndpoint::Responses,
                stream: true,
                store: false,
            },
            DomainConnectionTestEvent::Content {
                text: "OK".to_owned(),
            },
            DomainConnectionTestEvent::Completed {},
            DomainConnectionTestEvent::Failed {
                source: AccountProbeErrorSource::Upstream,
                gateway_error_code: GatewayErrorKind::RateLimited,
                send_state: Some(UpstreamSendState::Sent),
                message: "upstream unavailable".to_owned(),
                provider_error_code: Some("usage_exhausted".to_owned()),
                provider_error_type: Some("invalid_request_error".to_owned()),
                upstream_status: Some(429),
                upstream_content_type: Some("application/json".to_owned()),
                upstream_body: Some(r#"{"error":{"type":"usage_limit_reached"}}"#.to_owned()),
            },
        ]
        .map(|event| AccountConnectionTestEvent::from(event).data);

        assert_eq!(
            events,
            [
                json!({ "type": "test_start", "model": "grok-4.5", "interface": "responses", "text": "正在连接上游 Responses" }),
                json!({
                    "type": "request",
                    "interface": "responses",
                    "payload": {
                        "model": "grok-4.5",
                        "input": [{
                            "role": "user",
                            "content": [{
                                "type": "input_text",
                                "text": "Reply with exactly OK."
                            }]
                        }],
                        "stream": true,
                        "store": false
                    }
                }),
                json!({ "type": "content", "text": "OK" }),
                json!({ "type": "test_complete", "success": true }),
                json!({
                    "type": "error",
                    "source": "upstream",
                    "gatewayErrorCode": "rate_limited",
                    "sendState": "sent",
                    "error": "upstream unavailable",
                    "providerErrorCode": "usage_exhausted",
                    "providerErrorType": "invalid_request_error",
                    "upstreamStatus": 429,
                    "upstreamContentType": "application/json",
                    "upstreamBody": r#"{"error":{"type":"usage_limit_reached"}}"#
                }),
            ]
        );
    }

    #[test]
    fn provider_export_document_should_serialize_but_never_debug_secret() {
        let secret = "provider-refresh-token-must-not-enter-debug";
        let document = AccountExportData::new(json!({ "refresh_token": secret }));
        assert!(!format!("{document:?}").contains(secret));
        assert_eq!(
            serde_json::to_value(document).expect("serialize export"),
            json!({ "refresh_token": secret })
        );
    }
}

mod import_settings {
    use gateway_api::admin::accounts::{AccountImportRequest, CompleteAccountAuthorizationRequest};
    use serde_json::json;

    #[test]
    fn import_and_oauth_apply_the_same_settings_validation() {
        for (field, value) in [
            ("concurrencyLimit", json!(0)),
            ("concurrencyLimit", json!(4_294_967_296_u64)),
            ("weight", json!(0)),
            ("weight", json!(101)),
            ("groupIds", json!(["invalid-group"])),
        ] {
            let mut settings =
                json!({"enabled": false, "concurrencyLimit": null, "weight": 1, "groupIds": []});
            settings[field] = value;
            let import: AccountImportRequest = serde_json::from_value(
                json!({"provider": "openai", "data": {}, "settings": settings}),
            )
            .expect("import request");
            let oauth: CompleteAccountAuthorizationRequest = serde_json::from_value(json!({"provider": "xai", "flowId": "flow-test", "callbackUrl": "code", "settings": settings})).expect("OAuth request");
            assert_eq!(
                import.validate().expect_err("invalid settings").field(),
                field
            );
            assert_eq!(
                oauth.validate().expect_err("invalid settings").field(),
                field
            );
        }
    }

    #[test]
    fn import_settings_require_a_complete_explicit_configuration() {
        let settings =
            json!({"enabled": false, "concurrencyLimit": null, "weight": 100, "groupIds": []});
        let request: AccountImportRequest =
            serde_json::from_value(json!({"provider": "openai", "data": {}, "settings": settings}))
                .expect("settings");
        assert!(request.validate().is_ok());
        for field in ["enabled", "concurrencyLimit", "weight", "groupIds"] {
            let mut incomplete = settings.clone();
            incomplete
                .as_object_mut()
                .expect("settings object")
                .remove(field);
            assert!(
                serde_json::from_value::<AccountImportRequest>(
                    json!({"provider": "openai", "data": {}, "settings": incomplete})
                )
                .is_err(),
                "missing {field}"
            );
        }
    }
}
