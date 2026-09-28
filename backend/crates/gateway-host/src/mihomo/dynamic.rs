//! Port of upstream dynamic_subscription.go; malformed inputs never appear in errors.

use super::state::{Node, digest};
use gateway_admin::model::AdminError;
use reqwest::Url;
use serde_json::json;

pub(super) struct DynamicProxy {
    pub normalized: String,
    scheme: String,
    host: String,
    port: u16,
    username: String,
    password: String,
}

fn invalid() -> AdminError {
    AdminError::invalid("动态代理格式错误；支持主机:端口:用户名:密码或用户名:密码@主机:端口")
}

fn endpoint(value: &str) -> Option<(String, u16)> {
    let (host, port) = if let Some(value) = value.strip_prefix('[') {
        let (host, port) = value.split_once("]:")?;
        host.parse::<std::net::Ipv6Addr>().ok()?;
        (host, port)
    } else {
        let (host, port) = value.rsplit_once(':')?;
        if host.contains(':') {
            return None;
        }
        (host, port)
    };
    if host.is_empty()
        || host
            .chars()
            .any(|c| c.is_whitespace() || "/?#@%\\".contains(c))
    {
        return None;
    }
    let port = port.parse::<u16>().ok().filter(|p| *p != 0)?;
    Some((host.to_ascii_lowercase(), port))
}

fn candidate(
    scheme: &str,
    address: &str,
    credentials: &str,
    escaped: bool,
) -> Option<DynamicProxy> {
    let (host, port) = endpoint(address)?;
    let (username, password) = credentials.split_once(':')?;
    let decode = |s: &str| -> Option<String> {
        if escaped {
            // URL parsers preserve escaped user-info; decode before re-encoding once.
            let bytes = s.as_bytes();
            let mut out = Vec::with_capacity(bytes.len());
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%' {
                    let hex = s.get(i + 1..i + 3)?;
                    out.push(u8::from_str_radix(hex, 16).ok()?);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            String::from_utf8(out).ok()
        } else {
            Some(s.to_owned())
        }
    };
    let username = decode(username)?;
    let password = decode(password)?;
    if username.is_empty()
        || password.is_empty()
        || username
            .chars()
            .chain(password.chars())
            .any(|c| matches!(c, '\r' | '\n' | '\0'))
    {
        return None;
    }
    let authority = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let mut url = Url::parse(&format!("{scheme}://{authority}")).ok()?;
    url.set_username(&username).ok()?;
    url.set_password(Some(&password)).ok()?;
    let normalized = format!(
        "{scheme}://{}:{}@{authority}",
        url.username(),
        url.password()?
    );
    Some(DynamicProxy {
        normalized,
        scheme: scheme.into(),
        host,
        port,
        username,
        password,
    })
}

pub(super) fn parse(raw: &str) -> Result<DynamicProxy, AdminError> {
    let raw = raw.trim();
    let (scheme, value) = raw.split_once("://").unwrap_or(("http", raw));
    let scheme = scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https" | "socks5" | "socks5h") {
        return Err(invalid());
    }
    let mut candidates = Vec::new();
    for (index, _) in value.match_indices(':') {
        if let Some(p) = candidate(&scheme, &value[..index], &value[index + 1..], false) {
            candidates.push(p);
        }
        if let Some(p) = candidate(&scheme, &value[index + 1..], &value[..index], false) {
            candidates.push(p);
        }
    }
    if candidates.is_empty() {
        if let Some((user, host)) = value.rsplit_once('@')
            && let Some(p) = candidate(&scheme, host, user, true)
        {
            candidates.push(p);
        }
        if let Some((host, user)) = value.split_once('@')
            && let Some(p) = candidate(&scheme, host, user, true)
        {
            candidates.push(p);
        }
    }
    if candidates.len() != 1 {
        return Err(invalid());
    }
    Ok(candidates.remove(0))
}

impl DynamicProxy {
    pub fn node(&self) -> Node {
        let mut node = json!({
            "name": format!("DYNAMIC-{}", &digest(self.normalized.as_bytes())[..16]),
            "type": match self.scheme.as_str() { "https" => "http", "socks5h" => "socks5", other => other },
            "server": self.host, "port": self.port, "username": self.username, "password": self.password,
        }).as_object().expect("proxy object").clone();
        if self.scheme == "https" {
            node.insert("tls".into(), true.into());
        }
        node
    }
}

pub(super) fn normalize(raw: &[String]) -> Result<Vec<String>, AdminError> {
    let mut normalized = Vec::new();
    for (index, line) in raw.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let proxy = parse(line).map_err(|_| {
            AdminError::invalid(format!(
                "第 {} 行动态代理格式错误；请检查主机、端口及认证格式",
                index + 1
            ))
        })?;
        if !normalized.contains(&proxy.normalized) {
            normalized.push(proxy.normalized);
        }
    }
    Ok(normalized)
}
