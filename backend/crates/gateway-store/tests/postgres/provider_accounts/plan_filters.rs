use super::*;
use gateway_admin::model::accounts::AccountPlanFilter;

#[tokio::test]
async fn codex_plan_filters_match_aliases_before_paging_and_keep_provider_intersection() {
    let Some(database) = TestDatabase::create("codex_plan_filters").await else {
        return;
    };
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    let groups: &[&[&str]] = &[
        &["free", "free_workspace", "guest"],
        &["go"],
        &["plus"],
        &["prolite"],
        &["pro"],
        &["promax"],
        &["team", "self_serve_business_usage_based"],
        &["self_serve_business_prolite"],
        &[
            "business",
            "ent26",
            "enterprise",
            "hc",
            "enterprise_cbp_usage_based",
        ],
        &["enterprise_cbp_automation"],
        &["edu", "education"],
        &["edu_plus"],
        &["edu_pro"],
        &["sci"],
        &["sci_plus"],
    ];
    for aliases in groups {
        for raw in *aliases {
            let mut seed = account(&format!("acct_{raw}"), &format!("owner_{raw}"));
            seed.plan_type = Some((*raw).to_owned());
            repository.insert_provider_account(seed).await.unwrap();
        }
    }
    for (id, provider, raw) in [
        (
            "acct_premium_upper",
            "openai",
            "SELF_SERVE_BUSINESS_PROLITE",
        ),
        (
            "acct_premium_spaces",
            "openai",
            " self_serve_business_prolite ",
        ),
        ("acct_other_provider", "xai", "self_serve_business_prolite"),
        ("acct_future", "openai", "future-plan"),
        ("acct_unicode", "openai", "futur-étendu"),
    ] {
        let mut seed = account(id, &format!("owner_{id}"));
        seed.provider_kind = provider.to_owned();
        seed.plan_type = Some(raw.to_owned());
        repository.insert_provider_account(seed).await.unwrap();
    }
    let store = admin_account_store(&database.pool);
    let query = |filter| AccountListQuery {
        page: 1,
        page_size: PageSize::new(1).unwrap(),
        provider_kind: None,
        group_filter: None,
        search: None,
        status: None,
        plan_type: Some(filter),
        sort: Some(AccountSort {
            field: AccountSortField::CreatedAt,
            direction: SortDirection::Asc,
        }),
    };
    for aliases in groups {
        let filter = AccountPlanFilter::ProviderPlans {
            provider_kind: ProviderKind::new("openai").unwrap(),
            plan_types: aliases.iter().map(|raw| (*raw).to_owned()).collect(),
        };
        let mut expected = aliases
            .iter()
            .map(|raw| format!("acct_{raw}"))
            .collect::<Vec<_>>();
        if aliases[0] == "self_serve_business_prolite" {
            expected.extend([
                "acct_premium_upper".to_owned(),
                "acct_premium_spaces".to_owned(),
            ]);
        }
        let mut ids = Vec::new();
        for page_number in 1..=u32::try_from(expected.len()).unwrap() + 1 {
            let mut request = query(filter.clone());
            request.page = page_number;
            let result = store
                .list_accounts(request, Default::default())
                .await
                .unwrap();
            assert_eq!(result.total, expected.len() as u64, "{aliases:?}");
            ids.extend(
                result
                    .items
                    .iter()
                    .map(|item| item.account.id.as_str().to_owned()),
            );
        }
        ids.sort();
        expected.sort();
        assert_eq!(ids, expected, "{aliases:?}");
    }
    let premium = AccountPlanFilter::ProviderPlans {
        provider_kind: ProviderKind::new("openai").unwrap(),
        plan_types: vec!["self_serve_business_prolite".to_owned()],
    };
    let mut conflicting = query(premium.clone());
    conflicting.provider_kind = Some(ProviderKind::new("xai").unwrap());
    let result = store
        .list_accounts(conflicting, Default::default())
        .await
        .unwrap();
    assert_eq!(result.total, 0);
    assert!(result.items.is_empty());

    let mut searched = query(premium);
    searched.search = Some("acct_premium_upper".to_owned());
    let result = store
        .list_accounts(searched, Default::default())
        .await
        .unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.items[0].account.id.as_str(), "acct_premium_upper");

    for raw in ["business", "team", "future-plan"] {
        let result = store
            .list_accounts(query(raw.into()), Default::default())
            .await
            .unwrap();
        assert_eq!(result.total, 1, "legacy raw filter {raw}");
        let expected = if raw == "future-plan" {
            "acct_future".to_owned()
        } else {
            format!("acct_{raw}")
        };
        assert_eq!(result.items[0].account.id.as_str(), expected);
    }
    let unicode = store
        .list_accounts(query("FUTUR-ÉTENDU".into()), Default::default())
        .await
        .unwrap();
    assert_eq!(unicode.total, 1);
    assert_eq!(unicode.items[0].account.id.as_str(), "acct_unicode");
    database.close().await;
}
