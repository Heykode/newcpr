use super::{
    accounts::{account_record, context},
    relogin::{Harness, template_config, template_selection},
};
use gateway_admin::model::{
    AdminErrorKind, proxies::AccountProxySelection, relogin_templates::ReloginTemplateConfig,
};

fn import_command() -> gateway_admin::model::provider_credentials::ImportCredentials {
    use gateway_admin::model::provider_credentials::{ImportCredentials, ProviderDocument};
    ImportCredentials {
        context: context("template-import"),
        outbound_proxy_id: Some("proxy-input".into()),
        settings: None,
        document: ProviderDocument::new(gateway_core::account::OpaqueProviderData::new(
            serde_json::Map::new(),
        )),
    }
}

#[tokio::test]
async fn prefilled_import_keeps_edited_settings_exit_and_template_revision_fence() {
    use gateway_admin::model::accounts::ImportTemplateProxyMode;
    let h = Harness::new(vec![]).await;
    let service = h.services.account_templates();
    let mut config = template_config();
    config.enabled = false;
    config.weight = 9;
    let template = service.save_template(None, config).await.unwrap();
    for (proxy, clear, mode) in [
        (
            Some("edited-proxy"),
            false,
            ImportTemplateProxyMode::Replace,
        ),
        (None, true, ImportTemplateProxyMode::Replace),
        (None, false, ImportTemplateProxyMode::Preserve),
    ] {
        let mut command = import_command();
        let mut settings = template_config().settings().unwrap();
        settings.weight = gateway_core::account::AccountWeight::new(31).unwrap();
        settings.enabled = true;
        settings.clear_outbound_proxy = clear;
        settings.egress_mode = Some(None);
        command.outbound_proxy_id = proxy.map(str::to_owned);
        command.settings = Some(settings.clone());
        let prepared = service
            .prepare_prefilled_import(command, template_selection(&template))
            .await
            .unwrap();
        settings.template_proxy_mode = Some(mode);
        assert_eq!(prepared.settings, Some(settings));
        assert_eq!(prepared.outbound_proxy_id.as_deref(), proxy);
    }
    assert!(h.accounts.import_settings().is_empty());
    assert!(
        service
            .prepare_prefilled_import(import_command(), template_selection(&template))
            .await
            .is_err()
    );
    let mut command = import_command();
    command.settings = Some(template_config().settings().unwrap());
    service
        .delete_template(template_selection(&template))
        .await
        .unwrap();
    assert_eq!(
        service
            .prepare_prefilled_import(command, template_selection(&template))
            .await
            .unwrap_err()
            .kind(),
        AdminErrorKind::Conflict
    );
}

#[tokio::test]
async fn import_templates_freeze_the_complete_config_without_mutating_accounts() {
    let h = Harness::new(vec![account_record("openai")]).await;
    let service = h.services.account_templates();
    let mut config = template_config();
    config.model_access = Some(
        serde_json::from_value(serde_json::json!({
            "mode": "denylist", "models": ["model-template"]
        }))
        .unwrap(),
    );
    config.egress_mode = Some(None);
    config.responses_upstream = Some(gateway_core::account::ResponsesUpstream::Excel);
    config.excel_models_follow_global = Some(true);
    config.excel_ignore_encrypted_content = Some(true);
    config.excel_cache_creation_as_input = Some(false);
    let template = service.save_template(None, config.clone()).await.unwrap();
    let mut command = import_command();
    let mut settings = template_config().settings().unwrap();
    settings.custom_name = Some("Imported batch".into());
    let purchase = gateway_admin::model::account_purchase::AccountPurchaseUpdate {
        amount_cny: Some("50.125".into()),
        cycle_start: Some(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
    };
    settings.purchase_cost = Some(purchase.clone());
    settings.weight = gateway_core::account::AccountWeight::new(99).unwrap();
    command.settings = Some(settings);
    let prepared = service
        .prepare_import(command, template_selection(&template))
        .await
        .unwrap();
    let mut expected = config.settings().unwrap();
    expected.template_proxy_mode =
        Some(gateway_admin::model::accounts::ImportTemplateProxyMode::Replace);
    expected.custom_name = Some("Imported batch".into());
    expected.purchase_cost = Some(purchase);
    assert_eq!(prepared.settings, Some(expected.clone()));
    assert_eq!(prepared.outbound_proxy_id, None);
    assert!(h.accounts.import_settings().is_empty());
    assert!(h.accounts.batch_updates.lock().unwrap().is_empty());

    config.weight = 41;
    service
        .save_template(Some(template_selection(&template)), config)
        .await
        .unwrap();
    assert_eq!(
        prepared.settings,
        Some(expected),
        "queued settings are frozen"
    );
    assert_eq!(
        service
            .prepare_import(import_command(), template_selection(&template))
            .await
            .unwrap_err()
            .kind(),
        AdminErrorKind::Conflict
    );
}

#[tokio::test]
async fn import_templates_keep_legacy_optional_fields_and_explicit_proxy_semantics() {
    let h = Harness::new(vec![]).await;
    let service = h.services.account_templates();
    for (name, preserve, proxy) in [
        ("Direct", false, None),
        ("Preserve", true, None),
        ("Saved", false, Some("proxy-template".to_owned())),
    ] {
        let mut config = template_config();
        config.name = name.into();
        config.preserve_outbound_proxy = preserve;
        config.outbound_proxy_id = proxy.clone();
        config.turn_state_injection_enabled = None;
        config.concurrency_limit = None;
        config.group_ids.clear();
        let template = service.save_template(None, config.clone()).await.unwrap();
        let prepared = service
            .prepare_import(import_command(), template_selection(&template))
            .await
            .unwrap();
        let mut expected = config.settings().unwrap();
        expected.template_proxy_mode = Some(if preserve {
            gateway_admin::model::accounts::ImportTemplateProxyMode::Preserve
        } else {
            gateway_admin::model::accounts::ImportTemplateProxyMode::Replace
        });
        assert_eq!(prepared.settings, Some(expected));
        assert_eq!(prepared.outbound_proxy_id, proxy);
        let settings = prepared.settings.unwrap();
        assert_eq!(
            settings.clear_outbound_proxy,
            !preserve && config.outbound_proxy_id.is_none()
        );
        assert_eq!(settings.turn_state_injection_enabled, None);
        assert_eq!(settings.model_access, None);
        assert_eq!(settings.responses_upstream, None);
    }
}

#[tokio::test]
async fn import_templates_fail_closed_on_invalid_references_or_deleted_versions() {
    let h = Harness::new(vec![]).await;
    let service = h.services.account_templates();
    let template = service
        .save_template(None, template_config())
        .await
        .unwrap();
    *h.store.invalid_template_references.lock().unwrap() = true;
    assert_eq!(
        service
            .prepare_import(import_command(), template_selection(&template))
            .await
            .unwrap_err()
            .kind(),
        AdminErrorKind::Invalid
    );
    *h.store.invalid_template_references.lock().unwrap() = false;
    service
        .delete_template(template_selection(&template))
        .await
        .unwrap();
    assert_eq!(
        service
            .prepare_import(import_command(), template_selection(&template))
            .await
            .unwrap_err()
            .kind(),
        AdminErrorKind::Conflict
    );
    assert!(h.accounts.import_settings().is_empty());
}

#[test]
fn template_model_access_preserves_legacy_values_and_rejects_invalid_policies() {
    let legacy = serde_json::to_value(template_config()).unwrap();
    assert!(legacy.get("modelAccess").is_none());
    for policy in [None, Some(serde_json::Value::Null)] {
        let mut value = legacy.clone();
        if let Some(policy) = policy {
            value["modelAccess"] = policy;
        }
        let config: ReloginTemplateConfig = serde_json::from_value(value).unwrap();
        assert_eq!(config.model_access, None);
        assert_eq!(config.settings().unwrap().model_access, None);
        assert!(
            serde_json::to_value(config)
                .unwrap()
                .get("modelAccess")
                .is_none()
        );
    }
    for policy in [
        serde_json::json!({"mode":"allowlist","models":[]}),
        serde_json::json!({"mode":"denylist","models":["model-*"]}),
        serde_json::json!({"mode":"unknown","models":["model-a"]}),
    ] {
        let mut value = legacy.clone();
        value["modelAccess"] = policy;
        assert!(serde_json::from_value::<ReloginTemplateConfig>(value).is_err());
    }
}

#[tokio::test]
async fn template_model_access_roundtrips_and_applies_to_single_and_multiple_accounts() {
    let first = account_record("openai");
    let mut second = first.clone();
    second.id = "acct_second".into();
    let h = Harness::new(vec![first.clone(), second.clone()]).await;
    let service = h.services.account_templates();
    for mode in ["all", "allowlist", "denylist"] {
        let models = if mode == "all" {
            vec![]
        } else {
            vec!["model-a", "model-b"]
        };
        let policy =
            serde_json::from_value(serde_json::json!({"mode":mode,"models":models})).unwrap();
        let mut config = template_config();
        config.name = format!("Policy {mode}");
        config.model_access = Some(policy);
        let template = service.save_template(None, config.clone()).await.unwrap();
        assert_eq!(template.config.model_access, config.model_access);
        assert_eq!(
            service
                .resolve(template_selection(&template))
                .await
                .unwrap()
                .config,
            config
        );
        for ids in [
            vec![first.id.clone()],
            vec![first.id.clone(), second.id.clone()],
        ] {
            let before = h.accounts.batch_updates.lock().unwrap().len();
            service
                .apply(
                    ids.clone(),
                    template_selection(&template),
                    &context("template-model-access"),
                )
                .await
                .unwrap();
            let updates = h.accounts.batch_updates.lock().unwrap();
            assert_eq!(updates.len(), before + 1);
            assert_eq!(updates.last().unwrap().account_ids, ids);
            assert_eq!(updates.last().unwrap().model_access, config.model_access);
            assert_eq!(updates.last().unwrap().custom_name, None);
        }
    }
    assert!(h.accounts.rotation_attempts.lock().unwrap().is_empty());
    assert!(h.accounts.import_settings().is_empty());
}

#[tokio::test]
async fn template_preserve_outbound_is_explicit_and_legacy_null_still_means_direct() {
    let account = account_record("openai");
    let h = Harness::new(vec![account.clone()]).await;
    let service = h.services.account_templates();
    for preserve in [false, true] {
        let mut config = template_config();
        config.name = format!("Preserve proxy {preserve}");
        config.preserve_outbound_proxy = preserve;
        let template = service.save_template(None, config).await.unwrap();
        service
            .apply(
                vec![account.id.clone()],
                template_selection(&template),
                &context("preserve-egress"),
            )
            .await
            .unwrap();
        let updates = h.accounts.batch_updates.lock().unwrap();
        let update = updates.last().unwrap();
        assert_eq!(
            update.outbound_proxy,
            (!preserve).then_some(AccountProxySelection::Direct)
        );
        assert_eq!(update.egress_mode, None);
        assert_eq!(update.request_proxy_source, None);
    }
    let mut config = template_config();
    config.preserve_outbound_proxy = true;
    config.outbound_proxy_id = Some("proxy-test".into());
    assert!(config.settings().is_err());
    let mut legacy = serde_json::to_value(template_config()).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("preserveOutboundProxy");
    assert!(
        !serde_json::from_value::<ReloginTemplateConfig>(legacy)
            .unwrap()
            .preserve_outbound_proxy
    );
}

#[tokio::test]
async fn templates_apply_one_snapshot_to_one_or_many_accounts_without_credential_writes() {
    let first = account_record("openai");
    let mut second = first.clone();
    second.id = "acct_second".into();
    let h = Harness::new(vec![first.clone(), second.clone()]).await;
    let service = h.services.account_templates();
    for ids in [vec![first.id.clone()], vec![first.id, second.id]] {
        let mut config = template_config();
        config.name = format!("Selection {}", ids.len());
        config.outbound_proxy_id = Some("proxy-test".into());
        let template = service.save_template(None, config.clone()).await.unwrap();
        let result = service
            .apply(
                ids.clone(),
                template_selection(&template),
                &context("template-apply"),
            )
            .await
            .unwrap();
        assert_eq!(result.account_ids.len(), ids.len());
        let updates = h.accounts.batch_updates.lock().unwrap();
        let update = updates.last().unwrap();
        assert_eq!(update.custom_name, None, "templates never rename accounts");
        assert_eq!(
            update.model_access, None,
            "templates preserve account model policies"
        );
        let expected = config.settings().unwrap();
        assert_eq!(update.account_ids, ids);
        assert_eq!(update.enabled, Some(expected.enabled));
        assert_eq!(update.turn_state_injection_enabled, Some(true));
        assert_eq!(update.concurrency_limit, Some(expected.concurrency_limit));
        assert_eq!(update.weight, Some(expected.weight));
        assert_eq!(update.group_ids, Some(expected.group_ids));
        assert_eq!(
            update.outbound_proxy,
            Some(AccountProxySelection::Saved("proxy-test".into()))
        );
    }
    assert!(h.accounts.rotation_attempts.lock().unwrap().is_empty());
    assert!(h.accounts.import_settings().is_empty());
    assert_eq!(
        h.accounts.audit_requests(),
        ["template-apply", "template-apply"]
    );
}

#[tokio::test]
async fn templates_preserve_legacy_state_and_apply_explicit_off_and_empty_settings() {
    let account = account_record("openai");
    let h = Harness::new(vec![account.clone()]).await;
    let mut legacy = serde_json::to_value(template_config()).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("turnStateInjectionEnabled");
    let legacy: ReloginTemplateConfig = serde_json::from_value(legacy).unwrap();
    assert_eq!(legacy.turn_state_injection_enabled, None);
    for state in [None, Some(false), Some(true)] {
        let mut config = legacy.clone();
        config.name = format!("State {state:?}");
        config.turn_state_injection_enabled = state;
        config.group_ids.clear();
        config.concurrency_limit = None;
        let template = h
            .services
            .account_templates()
            .save_template(None, config)
            .await
            .unwrap();
        h.services
            .account_templates()
            .apply(
                vec![account.id.clone()],
                template_selection(&template),
                &context("apply"),
            )
            .await
            .unwrap();
        let updates = h.accounts.batch_updates.lock().unwrap();
        let update = updates.last().unwrap();
        assert_eq!(update.turn_state_injection_enabled, state);
        assert_eq!(update.group_ids, Some(vec![]));
        assert_eq!(update.concurrency_limit, Some(None));
        assert_eq!(update.outbound_proxy, Some(AccountProxySelection::Direct));
    }
}

#[tokio::test]
async fn templates_reject_stale_missing_invalid_and_unsupported_batches_before_mutation() {
    let openai = account_record("openai");
    let mut xai = account_record("xai");
    xai.id = "acct_xai".into();
    let h = Harness::new(vec![openai.clone(), xai.clone()]).await;
    let service = h.services.account_templates();
    let template = service
        .save_template(None, template_config())
        .await
        .unwrap();
    for ids in [
        vec![],
        vec![openai.id.clone(); 1001],
        vec![openai.id.clone(), openai.id.clone()],
        vec!["missing-account".into()],
        vec![openai.id.clone(), xai.id.clone()],
    ] {
        assert!(
            service
                .apply(ids, template_selection(&template), &context("reject"))
                .await
                .is_err()
        );
    }
    let mut stale = template_selection(&template);
    stale.revision += 1;
    assert_eq!(
        service
            .apply(vec![openai.id.clone()], stale, &context("reject"))
            .await
            .unwrap_err()
            .kind(),
        AdminErrorKind::Conflict,
    );
    *h.store.invalid_template_references.lock().unwrap() = true;
    assert!(
        service
            .apply(
                vec![openai.id.clone()],
                template_selection(&template),
                &context("reject")
            )
            .await
            .is_err()
    );
    *h.store.invalid_template_references.lock().unwrap() = false;
    service
        .delete_template(template_selection(&template))
        .await
        .unwrap();
    assert!(
        service
            .apply(
                vec![openai.id],
                template_selection(&template),
                &context("reject")
            )
            .await
            .is_err()
    );
    assert!(h.accounts.batch_updates.lock().unwrap().is_empty());
}

#[tokio::test]
async fn templates_allow_common_settings_for_mixed_providers_with_state_off() {
    let mut xai = account_record("xai");
    xai.id = "acct_xai".into();
    let accounts = vec![account_record("openai"), xai];
    let ids = accounts
        .iter()
        .map(|account| account.id.clone())
        .collect::<Vec<_>>();
    let h = Harness::new(accounts).await;
    let mut config = template_config();
    config.turn_state_injection_enabled = Some(false);
    let template = h
        .services
        .account_templates()
        .save_template(None, config)
        .await
        .unwrap();
    h.services
        .account_templates()
        .apply(
            ids.clone(),
            template_selection(&template),
            &context("mixed"),
        )
        .await
        .unwrap();
    assert_eq!(h.accounts.batch_updates.lock().unwrap()[0].account_ids, ids);
}
