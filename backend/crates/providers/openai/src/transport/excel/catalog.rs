//! Request-frozen tool catalogs; mutable session hints never own response history.

use super::{ClientTools, ExcelRequestError};
use gateway_core::{account::OpaqueProviderData, provider_ports::ProviderReplayPort};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(2);
const MAX_BYTES: usize = 1024 * 1024;

pub(super) async fn resolve(
    store: &dyn ProviderReplayPort,
    owner: &str,
    session: Option<&str>,
    inherited: Option<&Value>,
    source: &Map<String, Value>,
) -> Result<(ClientTools, Value), ExcelRequestError> {
    let key = session
        .filter(|s| !s.trim().is_empty() && s.len() <= 4096)
        .map(|session| {
            hex::encode(Sha256::digest(
                json!(["cpr-excel-catalog-v1", owner, session])
                    .to_string()
                    .as_bytes(),
            ))
        });
    let explicit = source.contains_key("tools");
    let additional = source
        .get("input")
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(|item| item["type"] == "additional_tools"));
    for _ in 0..8 {
        let previous = match key
            .as_ref()
            .filter(|_| explicit || additional || inherited.is_none())
        {
            Some(key) => tokio::time::timeout(TIMEOUT, store.read_catalog(key))
                .await
                .ok()
                .and_then(Result::ok)
                .flatten(),
            None => None,
        };
        let mut effective = source.clone();
        if !explicit
            && let Some(catalog) = inherited.or_else(|| {
                previous
                    .as_ref()
                    .and_then(|v| v.expose_to_provider().get("tools"))
            })
        {
            effective.insert("tools".into(), catalog.clone());
        }
        // `none` controls this turn, not the reusable declaration set.
        let choice = effective.remove("tool_choice");
        let tools = ClientTools::parse(&effective)?;
        let catalog = tools.catalog();
        if serde_json::to_vec(&catalog)
            .map_err(|_| ExcelRequestError::Tool)?
            .len()
            > MAX_BYTES - 128
        {
            return Err(ExcelRequestError::Tool);
        }
        let tools = tools.with_choice(choice.as_ref())?;
        let Some(key) = key.as_ref().filter(|_| explicit || additional) else {
            return Ok((tools, catalog));
        };
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).map_err(|_| ExcelRequestError::Tool)?;
        let payload = OpaqueProviderData::new(
            json!({"version":hex::encode(nonce),"tools":catalog})
                .as_object()
                .unwrap()
                .clone(),
        );
        let written = tokio::time::timeout(
            TIMEOUT,
            store.compare_exchange_catalog(key, previous.as_ref(), &payload),
        )
        .await;
        // Explicit requests keep their own catalog even when a newer writer wins.
        // Only a delta-only update retries against a fresh snapshot.
        if explicit || inherited.is_some() || !matches!(written, Ok(Ok(false))) {
            return Ok((tools, catalog));
        }
    }
    Err(ExcelRequestError::CatalogConflict)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::BoxFuture;
    use gateway_core::provider_ports::ProviderStoreError;

    #[tokio::test]
    async fn duplicate_annotations_preserve_current_and_inherited_catalogs() {
        use super::super::tests::MemoryReplay;
        for inherited in [false, true] {
            let store = MemoryReplay::default();
            let current = json!({"type":"namespace","name":"workspace","tools":[{
                "type":"custom","name":"patch","description":"Current contract",
                "format":{"type":"text"},"encrypted":false,"strict":true
            }]});
            let initial = json!({"tools":[current.clone()],"input":"begin"});
            let (_, expected) = resolve(
                &store,
                "owner",
                Some("session"),
                None,
                initial.as_object().unwrap(),
            )
            .await
            .unwrap();
            let mut historical = current.clone();
            historical["tools"][0]["description"] = "Historical description".into();
            historical["tools"][0]["defer_loading"] = true.into();
            let mut source = json!({"input":[{"type":"additional_tools","tools":[historical]},{"role":"user","content":"continue"}]});
            if !inherited {
                source["tools"] = json!([current]);
            }
            let original = source.clone();
            let (tools, catalog) = resolve(
                &store,
                "owner",
                Some("session"),
                None,
                source.as_object().unwrap(),
            )
            .await
            .unwrap();
            assert_eq!(source, original);
            assert_eq!(catalog, expected);
            assert!(tools.instructions().contains("Current contract"));
            assert!(!tools.instructions().contains("Historical description"));
            let (_, stored) = resolve(
                &store,
                "owner",
                Some("session"),
                None,
                json!({"input":"next"}).as_object().unwrap(),
            )
            .await
            .unwrap();
            assert_eq!(stored, expected);
        }
    }

    #[tokio::test]
    async fn conflicting_historical_contract_never_mutates_cached_catalog() {
        use super::super::tests::MemoryReplay;
        let store = MemoryReplay::default();
        let declaration = json!({"type":"custom","name":"patch","format":{"type":"text"},"strict":true,"encrypted":false});
        let initial = json!({"tools":[declaration.clone()],"input":"begin"});
        let (_, expected) = resolve(
            &store,
            "owner",
            Some("session"),
            None,
            initial.as_object().unwrap(),
        )
        .await
        .unwrap();
        for (field, changed) in [
            ("type", json!("function")),
            ("format", json!({"type":"grammar"})),
            ("strict", json!(false)),
            ("parameters", json!({"type":"object"})),
            ("encrypted", json!(true)),
            ("new_constraint", json!(true)),
        ] {
            let mut conflicting = declaration.clone();
            conflicting[field] = changed;
            let source = json!({"input":[{"type":"additional_tools","tools":[conflicting]}]});
            assert!(
                matches!(
                    resolve(
                        &store,
                        "owner",
                        Some("session"),
                        None,
                        source.as_object().unwrap()
                    )
                    .await,
                    Err(ExcelRequestError::Tool)
                ),
                "{field}"
            );
            let (_, stored) = resolve(
                &store,
                "owner",
                Some("session"),
                None,
                json!({"input":"next"}).as_object().unwrap(),
            )
            .await
            .unwrap();
            assert_eq!(stored, expected, "{field}");
        }
    }

    struct Contended;
    impl ProviderReplayPort for Contended {
        fn read<'a>(
            &'a self,
            _: &'a str,
        ) -> BoxFuture<'a, Result<Option<OpaqueProviderData>, ProviderStoreError>> {
            Box::pin(async { Ok(None) })
        }
        fn write<'a>(
            &'a self,
            _: &'a str,
            _: &'a OpaqueProviderData,
        ) -> BoxFuture<'a, Result<(), ProviderStoreError>> {
            Box::pin(async { Ok(()) })
        }
        fn compare_exchange_catalog<'a>(
            &'a self,
            _: &'a str,
            _: Option<&'a OpaqueProviderData>,
            _: &'a OpaqueProviderData,
        ) -> BoxFuture<'a, Result<bool, ProviderStoreError>> {
            Box::pin(async { Ok(false) })
        }
    }

    #[tokio::test]
    async fn catalog_conflict_is_not_a_successful_delta_merge() {
        let delta = json!({"input":[{"type":"additional_tools","tools":[{"type":"function","name":"read"}]}]});
        assert!(matches!(
            resolve(
                &Contended,
                "owner",
                Some("thread"),
                None,
                delta.as_object().unwrap()
            )
            .await,
            Err(ExcelRequestError::CatalogConflict)
        ));
        let explicit = json!({"tools":[{"type":"function","name":"read"}],"input":"hello"});
        assert!(
            resolve(
                &Contended,
                "owner",
                Some("thread"),
                None,
                explicit.as_object().unwrap()
            )
            .await
            .is_ok()
        );
        assert!(
            resolve(
                &gateway_core::provider_ports::UnavailableProviderReplay,
                "owner",
                Some("thread"),
                None,
                delta.as_object().unwrap()
            )
            .await
            .is_ok()
        );
    }
}
