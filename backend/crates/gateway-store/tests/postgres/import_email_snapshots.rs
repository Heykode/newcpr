use chrono::{TimeDelta, Utc};
use gateway_admin::{
    model::{
        MutationActor, MutationContext,
        provider_credentials::{
            CredentialImportCommit, PreparedCredentialCreate, PreparedCredentialImport,
            ProviderDocument,
        },
    },
    ports::store::AccountStore,
};
use gateway_core::{
    account::{CredentialState, OpaqueProviderData, ProviderAccountId},
    routing::ProviderKind,
};
use gateway_store::postgres::{PgProviderAccountRepository, ProviderAccountRepository};
use serde_json::json;

use super::{TestDatabase, admin_account_store};

#[tokio::test]
async fn admin_import_email_snapshots_use_committed_ids_and_final_duplicate_values() {
    let Some(database) = TestDatabase::create("import_email_snapshots").await else {
        return;
    };
    let store = admin_account_store(&database.pool);
    let context = MutationContext {
        actor: MutationActor::System,
        request_id: "import-email-snapshots".to_owned(),
    };
    let prepare = |id: &str, user: &str, email: Option<&str>| PreparedCredentialCreate {
        outbound_proxy: None,
        account_id: ProviderAccountId::new(id).unwrap(),
        provider_kind: ProviderKind::new("openai").unwrap(),
        name: id.to_owned(),
        email: email.map(str::to_owned),
        upstream_user_id: Some(user.to_owned()),
        upstream_account_id: Some(format!("workspace-{user}")),
        plan_type: Some("pro".to_owned()),
        authentication_kind: "oauth".to_owned(),
        provider_material: ProviderDocument::new(OpaqueProviderData::new(
            [("access_token".to_owned(), json!("synthetic-import-value"))]
                .into_iter()
                .collect(),
        )),
        has_refresh_token: true,
        access_token_expires_at: Some(Utc::now() + TimeDelta::hours(1)),
        next_refresh_at: None,
        enabled: true,
        credential_state: CredentialState::Ready,
        credential_observed_at: Utc::now(),
    };
    let command = |credentials| CredentialImportCommit {
        outbound_proxy: None,
        settings: None,
        prepared: PreparedCredentialImport {
            provider_kind: ProviderKind::new("openai").unwrap(),
            credentials,
        },
    };
    let initial = store
        .commit_credential_import(
            command(vec![
                prepare("acct_email_first", "first", Some("first@example.com")),
                prepare("acct_email_second", "second", Some("second@example.com")),
            ]),
            &context,
        )
        .await
        .unwrap();
    assert_eq!(
        initial.credential_emails[&initial.credential_ids[0]].as_deref(),
        Some("first@example.com")
    );
    let imported = store
        .commit_credential_import(
            command(vec![
                prepare("acct_email_alias", "first", Some("updated@example.com")),
                prepare("acct_email_second_alias", "second", None),
                prepare("acct_email_last_alias", "first", Some("final@example.com")),
                prepare("acct_email_missing", "third", None),
            ]),
            &context,
        )
        .await
        .unwrap();
    assert_eq!(
        imported
            .credential_ids
            .iter()
            .map(ProviderAccountId::as_str)
            .collect::<Vec<_>>(),
        [
            "acct_email_first",
            "acct_email_second",
            "acct_email_first",
            "acct_email_missing"
        ]
    );
    assert_eq!(imported.credential_emails.len(), 3);
    assert_eq!(
        imported.credential_emails[&imported.credential_ids[0]].as_deref(),
        Some("final@example.com")
    );
    assert_eq!(
        imported.credential_emails[&imported.credential_ids[1]],
        None
    );
    assert_eq!(
        imported.credential_emails[&imported.credential_ids[3]],
        None
    );
    let repository = PgProviderAccountRepository::new(database.pool.clone());
    for (id, email) in imported.credential_emails {
        let saved = repository
            .load_provider_account(id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.summary.email, email);
    }
    database.close().await;
}
