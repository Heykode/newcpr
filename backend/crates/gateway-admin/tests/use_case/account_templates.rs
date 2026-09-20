use super::{
    accounts::{account_record, context},
    relogin::{Harness, template_config, template_selection},
};
use gateway_admin::model::{
    AdminErrorKind, proxies::AccountProxySelection, relogin_templates::ReloginTemplateConfig,
};

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
