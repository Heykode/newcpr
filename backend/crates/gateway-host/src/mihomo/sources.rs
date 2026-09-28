//! Subscription providers own outbound definitions only, never listeners or rules.

use super::{
    dynamic,
    state::{Node, Saved, Subscription, SubscriptionCache, digest, node_id, node_name},
};
use chrono::Utc;
use gateway_admin::model::{
    AdminError,
    mihomo::{MihomoAction, MihomoCommand, SubscriptionDownloadMode},
};
use reqwest::{Client, Url};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const MAX_SUBSCRIPTIONS: usize = 32;
const MAX_NODES: usize = 1000;
const MAX_SOURCE_BYTES: usize = 4 << 20;

pub(super) fn normalize_urls(raw: &[String]) -> Result<Vec<String>, AdminError> {
    if raw.len() > MAX_SUBSCRIPTIONS {
        return Err(AdminError::invalid("最多32个订阅（与上游一致）"));
    }
    let mut result = Vec::new();
    for raw in raw {
        let address = raw.trim();
        if address.is_empty() {
            continue;
        }
        let url =
            Url::parse(address).map_err(|_| AdminError::invalid("订阅地址必须是HTTP(S)地址"))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || address.len() > 8192
        {
            return Err(AdminError::invalid(
                "订阅地址不能包含登录信息或片段，且最长8192字节",
            ));
        }
        if !result.iter().any(|item| item == address) {
            result.push(address.to_owned());
        }
    }
    Ok(result)
}

pub(super) fn source_action(action: MihomoAction) -> bool {
    matches!(
        action,
        MihomoAction::SubscriptionAdd
            | MihomoAction::SubscriptionUpdate
            | MihomoAction::SubscriptionRefresh
            | MihomoAction::SubscriptionRemove
            | MihomoAction::SubscriptionEnable
            | MihomoAction::SubscriptionDisable
            | MihomoAction::DynamicAppend
            | MihomoAction::DynamicReplace
            | MihomoAction::DynamicRemove
            | MihomoAction::DynamicClear
    )
}

pub(super) fn prepare(next: &mut Saved, command: &MihomoCommand) -> Result<(), AdminError> {
    use MihomoAction::*;
    match command.action {
        SubscriptionAdd => {
            let urls = normalize_urls(&command.subscriptions)?;
            if urls.is_empty() {
                return Err(AdminError::invalid("请提供订阅地址"));
            }
            for url in urls {
                if !next.subscriptions.iter().any(|s| s.url == url) {
                    next.subscriptions.push(Subscription {
                        url,
                        label: command.name.trim().to_owned(),
                        enabled: true,
                        cache: None,
                    });
                }
            }
            if next.subscriptions.len() > MAX_SUBSCRIPTIONS {
                return Err(AdminError::invalid("最多32个订阅（与上游一致）"));
            }
        }
        SubscriptionUpdate | SubscriptionRefresh | SubscriptionRename | SubscriptionRemove
        | SubscriptionEnable | SubscriptionDisable => {
            let index = next
                .subscriptions
                .iter()
                .position(|s| digest(s.url.as_bytes()) == command.target)
                .ok_or_else(|| AdminError::invalid("订阅不存在或已更新，请刷新列表"))?;
            if command.action == SubscriptionRemove {
                next.subscriptions.remove(index);
                return Ok(());
            }
            let source = &mut next.subscriptions[index];
            match command.action {
                SubscriptionUpdate => {
                    let urls = normalize_urls(&command.subscriptions)?;
                    if urls.len() != 1 {
                        return Err(AdminError::invalid("更新订阅需要一个地址"));
                    }
                    source.url = urls[0].clone();
                    source.cache = None;
                }
                SubscriptionRefresh => {
                    if !source.enabled {
                        return Err(AdminError::invalid("请先启用订阅再刷新"));
                    }
                    source.cache = None;
                }
                SubscriptionRename => source.label = command.name.trim().to_owned(),
                SubscriptionEnable => source.enabled = true,
                SubscriptionDisable => source.enabled = false,
                _ => {}
            }
            let ids: BTreeSet<_> = next.subscriptions.iter().map(|s| &s.url).collect();
            if ids.len() != next.subscriptions.len() {
                return Err(AdminError::invalid("该订阅地址已存在"));
            }
        }
        DynamicAppend | DynamicReplace => {
            let parsed = dynamic::normalize(&command.dynamic_proxies)?;
            if parsed.is_empty() {
                return Err(AdminError::invalid(
                    "请提供动态代理；清空请使用明确的清空操作",
                ));
            }
            if command.action == DynamicReplace {
                next.dynamic_proxies = parsed;
            } else {
                for value in parsed {
                    if !next.dynamic_proxies.contains(&value) {
                        next.dynamic_proxies.push(value);
                    }
                }
            }
        }
        DynamicRemove => {
            let index = next
                .dynamic_proxies
                .iter()
                .position(|raw| {
                    dynamic::parse(raw).is_ok_and(|p| node_name(&p.node()) == command.target)
                })
                .ok_or_else(|| AdminError::invalid("动态代理不存在"))?;
            next.dynamic_proxies.remove(index);
        }
        DynamicClear => next.dynamic_proxies.clear(),
        _ => {}
    }
    Ok(())
}

pub(super) async fn read_limited(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, AdminError> {
    use futures::StreamExt;
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(AdminError::invalid("下载响应超过允许大小"));
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| AdminError::internal("下载响应中断"))?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(AdminError::invalid("下载响应超过允许大小"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub(super) fn parse_subscription(bytes: &[u8]) -> Result<SubscriptionCache, AdminError> {
    let text = std::str::from_utf8(bytes).map_err(|_| AdminError::invalid("订阅不是UTF-8 YAML"))?;
    let value: Value = config::Config::builder()
        .add_source(config::File::from_str(text, config::FileFormat::Yaml))
        .build()
        .and_then(config::Config::try_deserialize)
        .map_err(|_| AdminError::invalid("订阅不是有效的代理YAML"))?;
    let entries = value
        .get("proxies")
        .and_then(Value::as_array)
        .ok_or_else(|| AdminError::invalid("订阅缺少proxies节点列表"))?;
    let mut nodes = Vec::new();
    let mut names = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for value in entries {
        let mut node = value
            .as_object()
            .ok_or_else(|| AdminError::invalid("订阅节点结构无效"))?
            .clone();
        let kind = node.get("type").and_then(Value::as_str).unwrap_or("");
        if kind.is_empty()
            || kind.eq_ignore_ascii_case("direct")
            || kind.eq_ignore_ascii_case("reject")
        {
            continue;
        }
        let label = node
            .remove("name")
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        node.remove("dialer-proxy");
        let id = node_id(&node);
        if !seen.insert(id.clone()) {
            continue;
        }
        let name = format!("node-{}", &id[..16]);
        names.insert(name.clone(), display_name(&label));
        node.insert("name".into(), name.into());
        nodes.push(node);
        if nodes.len() > MAX_NODES {
            return Err(AdminError::invalid("最多1000个订阅节点（与上游一致）"));
        }
    }
    if nodes.is_empty() {
        return Err(AdminError::invalid("订阅没有可用节点"));
    }
    Ok(SubscriptionCache {
        nodes,
        names,
        updated_at: Utc::now(),
    })
}

fn display_name(value: &str) -> String {
    // A provider may put a subscription URL or user-info in its display label.
    if value.contains("://") || value.contains('@') {
        return "订阅节点".into();
    }
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(160)
        .collect()
}

async fn download(client: &Client, address: &str) -> Result<SubscriptionCache, AdminError> {
    let response = client
        .get(address)
        .header("User-Agent", "clash.meta")
        .send()
        .await
        .map_err(|_| AdminError::internal("订阅下载失败"))?;
    if !response.status().is_success() {
        return Err(AdminError::invalid("订阅服务器返回非成功状态"));
    }
    parse_subscription(&read_limited(response, MAX_SOURCE_BYTES).await?)
}

pub(super) async fn resolve(
    next: &mut Saved,
    direct: &Client,
    proxy: Option<&Client>,
) -> Result<(), AdminError> {
    let mut nodes: Vec<Node> = Vec::new();
    let mut names = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for source in next.subscriptions.iter_mut().filter(|s| s.enabled) {
        if source.cache.is_none() {
            let cache = match next.download_mode {
                SubscriptionDownloadMode::Direct => download(direct, &source.url).await?,
                SubscriptionDownloadMode::Proxy => {
                    download(
                        proxy.ok_or_else(|| AdminError::invalid("订阅下载代理尚未运行"))?,
                        &source.url,
                    )
                    .await?
                }
                SubscriptionDownloadMode::Auto => match proxy {
                    Some(proxy) => match download(proxy, &source.url).await {
                        Ok(cache) => cache,
                        Err(_) => download(direct, &source.url).await?,
                    },
                    None => download(direct, &source.url).await?,
                },
            };
            source.cache = Some(cache);
        }
        if let Some(cache) = &source.cache {
            for node in &cache.nodes {
                if seen.insert(node_id(node)) {
                    nodes.push(node.clone());
                }
            }
            names.extend(cache.names.clone());
        }
        if nodes.len() > MAX_NODES {
            return Err(AdminError::invalid("最多1000个订阅节点（与上游一致）"));
        }
    }
    for raw in &next.dynamic_proxies {
        let node = dynamic::parse(raw)?.node();
        if seen.insert(node_id(&node)) {
            nodes.push(node);
        }
    }
    next.nodes = nodes;
    next.names = names;
    Ok(())
}
