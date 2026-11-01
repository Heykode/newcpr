//! Provider-owned serialization for frozen request configuration.

use gateway_core::account::OpaqueProviderData;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub fn serialize<S: serde::Serializer>(
    value: &OpaqueProviderData,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    value.expose_to_provider().serialize(serializer)
}

pub fn deserialize<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<OpaqueProviderData, D::Error> {
    Map::<String, Value>::deserialize(deserializer).map(OpaqueProviderData::new)
}
