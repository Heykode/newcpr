use super::QualityRuleConfig;
use gateway_core::account::{Excel403Action, ExcelModels, ResponsesUpstream};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

/// Read-only settings from a completed run. Never use a history snapshot as a rule.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(try_from = "Value")]
pub struct QualityRunConfig(Value);

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecordedRecovery {
    enabled: bool,
    interval_minutes: u32,
}

fn copy_recorded<T: DeserializeOwned + Serialize>(
    source: &Value,
    target: &mut Value,
    key: &str,
) -> serde_json::Result<()> {
    if let Some(value) = source.get(key) {
        target[key] = serde_json::to_value(Option::<T>::deserialize(value)?)?;
    }
    Ok(())
}

impl TryFrom<Value> for QualityRunConfig {
    type Error = serde_json::Error;

    fn try_from(saved: Value) -> Result<Self, Self::Error> {
        // Keep normal field validation and defaults. Only the recorded, typed
        // historical fields may be added back to the administrator response.
        let mut view = serde_json::to_value(QualityRuleConfig::deserialize(&saved)?)?;
        if saved["failureAction"] == "enable_excel" {
            view["failureAction"] = saved["failureAction"].clone();
        }
        copy_recorded::<bool>(&saved, &mut view, "disableExcelOnNativeRecovery")?;
        copy_recorded::<u8>(&saved, &mut view, "excelRecoveryThreshold")?;
        if let (Some(source), Some(target)) = (
            saved.pointer("/failureTemplate/config"),
            view.pointer_mut("/failureTemplate/config"),
        ) {
            copy_recorded::<ResponsesUpstream>(source, target, "responsesUpstream")?;
            copy_recorded::<ExcelModels>(source, target, "excelModels")?;
            copy_recorded::<Excel403Action>(source, target, "excel403Action")?;
            copy_recorded::<RecordedRecovery>(source, target, "excelRecovery")?;
            for key in [
                "excelModelsFollowGlobal",
                "excelCacheCreationAsInput",
                "excelIgnoreEncryptedContent",
                "excelAutoDisableOn403",
            ] {
                copy_recorded::<bool>(source, target, key)?;
            }
        }
        Ok(Self(view))
    }
}
