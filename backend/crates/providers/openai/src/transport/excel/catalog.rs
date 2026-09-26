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
    for attempt in 0..4 {
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
        if explicit || inherited.is_some() || attempt == 3 || !matches!(written, Ok(Ok(false))) {
            return Ok((tools, catalog));
        }
    }
    unreachable!("bounded catalog loop always returns")
}
