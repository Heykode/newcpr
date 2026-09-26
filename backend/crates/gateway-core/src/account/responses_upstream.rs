//! Account-selected Responses protocol, independent of credentials and affinity.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<String>", into = "Vec<String>")]
pub struct ExcelModels(Vec<String>);

impl Default for ExcelModels {
    fn default() -> Self {
        Self(vec![
            "gpt-6-astra".into(),
            "gpt-5.6-sol".into(),
            "gpt-5.6-terra".into(),
        ])
    }
}

impl TryFrom<Vec<String>> for ExcelModels {
    type Error = &'static str;

    fn try_from(values: Vec<String>) -> Result<Self, Self::Error> {
        if values.len() > 64 {
            return Err("Excel supports at most 64 configured models");
        }
        let mut models = Vec::with_capacity(values.len());
        for value in values {
            let model = value.trim();
            if model.is_empty()
                || model.len() > 128
                || !model
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'-' | b'_' | b'.'))
            {
                return Err("invalid Excel model identifier");
            }
            if !models.iter().any(|existing| existing == model) {
                models.push(model.to_owned());
            }
        }
        Ok(Self(models))
    }
}

impl From<ExcelModels> for Vec<String> {
    fn from(models: ExcelModels) -> Self {
        models.0
    }
}

impl ExcelModels {
    #[must_use]
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }

    #[must_use]
    pub fn contains(&self, model: &str) -> bool {
        self.0.iter().any(|candidate| candidate == model)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponsesUpstream {
    #[default]
    Codex,
    Excel,
}

impl ResponsesUpstream {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Excel => "excel",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "codex" => Some(Self::Codex),
            "excel" => Some(Self::Excel),
            _ => None,
        }
    }

    #[must_use]
    pub fn supports_account(self, provider: &str, authentication_kind: &str) -> bool {
        self == Self::Codex || (provider == "openai" && authentication_kind == "oauth")
    }
}
