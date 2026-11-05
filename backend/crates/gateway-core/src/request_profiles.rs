//! Key 使用完整画像，或显式继承全局画像并附加 Provider 所有的字段

use crate::account::OpaqueProviderData;
use serde_json::{Map, Value};

/// `inherit` 是 Key 配置的外层协议；内部字段名和值只由对应 Provider 校验
#[must_use]
pub fn inherited_fields(key: &OpaqueProviderData) -> Option<&Map<String, Value>> {
    let document = key.expose_to_provider();
    (document.len() == 1)
        .then(|| document.get("inherit")?.as_object())
        .flatten()
}
