//! Explicit Excel image transport settings; never participate in account selection.

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExcelImageTransport {
    Native {},
    Relay {
        #[serde(rename = "publicUrl")]
        public_url: String,
    },
}

impl ExcelImageTransport {
    #[must_use]
    pub fn validate(&self) -> bool {
        match self {
            Self::Native {} => true,
            Self::Relay { public_url } => Self::valid_public_url(public_url),
        }
    }

    #[must_use]
    pub fn valid_public_url(value: &str) -> bool {
        value.len() <= 2048
            && value.trim() == value
            && !value.chars().any(char::is_control)
            && !value.contains('\\')
            && url::Url::parse(value).is_ok_and(|url| {
                url.scheme() == "https"
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none()
                    && url.path() == "/"
                    && url.host_str().is_some_and(|host| {
                        matches!(url.host(), Some(url::Host::Domain(_)))
                            && host.contains('.')
                            && host != "localhost"
                            && !host.ends_with(".localhost")
                            && !host.ends_with(".local")
                    })
            })
    }
}
