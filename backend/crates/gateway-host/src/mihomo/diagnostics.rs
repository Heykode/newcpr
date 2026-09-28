//! Administrator probes use an isolated, authenticated kernel, never production rules.

use super::{
    files,
    sources::read_limited,
    state::{COUNTRY_CODES, CountryObservation, Node, node_name},
};
use chrono::Utc;
use gateway_admin::model::{
    AdminError,
    mihomo::{MihomoNodeCheck, MihomoQualityItem, MihomoQualityReport},
};
use serde::Deserialize;
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

struct IsolatedKernel {
    dir: PathBuf,
    child: Option<tokio::process::Child>,
}
impl Drop for IsolatedKernel {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.start_kill();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn isolated(
    dir: &Path,
    node: &Node,
    build: &super::ClientBuilder,
) -> Result<(IsolatedKernel, reqwest::Client), AdminError> {
    let secret = files::secret()?;
    let root = dir.join(format!(".node-probe-{}", &secret[..16]));
    files::prepare_dir(&root)?;
    let mut guard = IsolatedKernel {
        dir: root.clone(),
        child: None,
    };
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|_| AdminError::internal("无法分配节点检测端口"))?;
    let address = listener
        .local_addr()
        .map_err(|_| AdminError::internal("无法读取节点检测端口"))?;
    drop(listener);
    let config = json!({
        "mixed-port":address.port(),"allow-lan":false,"bind-address":"127.0.0.1","mode":"rule","log-level":"silent",
        "authentication":[format!("probe:{secret}")],"proxies":[node],"rules":[format!("MATCH,{}",node_name(node))]
    });
    let path = root.join("config.json");
    files::atomic_write(&path, config.to_string().as_bytes(), false)?;
    guard.child = Some(
        super::kernel::command(&dir.join("mihomo"))
            .arg("-d")
            .arg(&root)
            .arg("-f")
            .arg(path)
            .spawn()
            .map_err(|_| AdminError::internal("隔离节点内核启动失败"))?,
    );
    let ready = async {
        loop {
            if !matches!(
                guard.child.as_mut().expect("started kernel").try_wait(),
                Ok(None)
            ) {
                return Err(AdminError::internal("隔离节点内核启动后退出"));
            }
            if tokio::net::TcpStream::connect(address).await.is_ok() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(5), ready)
        .await
        .map_err(|_| AdminError::internal("隔离节点内核启动超时"))??;
    let proxy = reqwest::Proxy::all(format!("http://probe:{secret}@{address}"))
        .map_err(|_| AdminError::internal("节点检测代理配置失败"))?;
    let client = build(
        reqwest::Client::builder()
            .no_proxy()
            .proxy(proxy)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(15)),
    )?;
    Ok((guard, client))
}

async fn exit_info(client: &reqwest::Client) -> Result<(String, String), AdminError> {
    #[derive(Deserialize)]
    struct Exit {
        ip: std::net::IpAddr,
        country: String,
    }
    let response = client
        .get("https://api.country.is/")
        .timeout(Duration::from_secs(6))
        .send()
        .await
        .map_err(|_| AdminError::internal("出口地址检测失败"))?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(AdminError::internal("出口地址检测返回异常状态"));
    }
    let result: Exit = serde_json::from_slice(&read_limited(response, 8192).await?)
        .map_err(|_| AdminError::internal("出口地址检测响应无效"))?;
    let country = result.country.trim().to_ascii_uppercase();
    if !COUNTRY_CODES.split_whitespace().any(|c| c == country) {
        return Err(AdminError::internal("出口地区未知"));
    }
    Ok((result.ip.to_string(), country))
}

pub(super) async fn country(
    dir: &Path,
    node: &Node,
    build: &super::ClientBuilder,
) -> CountryObservation {
    let result = tokio::time::timeout(Duration::from_secs(12), async {
        let (_guard, client) = isolated(dir, node, build).await?;
        exit_info(&client).await
    })
    .await;
    match result {
        Ok(Ok((_, code))) => CountryObservation {
            code: Some(code),
            checked_at: Some(Utc::now()),
            error: None,
        },
        _ => CountryObservation {
            code: None,
            checked_at: Some(Utc::now()),
            error: Some("lookup_failed".into()),
        },
    }
}

pub(super) async fn probe(
    dir: &Path,
    node: &Node,
    build: &super::ClientBuilder,
) -> Result<bool, AdminError> {
    let (_guard, client) = isolated(dir, node, build).await?;
    // Match the upstream delay probe, not the independent geography service.
    Ok(client
        .get("https://www.gstatic.com/generate_204")
        .timeout(Duration::from_secs(1))
        .send()
        .await
        .is_ok_and(|response| response.status().is_success()))
}

pub(super) fn parse_exit(body: &[u8], detailed: bool) -> Option<MihomoNodeCheck> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    if detailed
        && !value["status"]
            .as_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("success"))
    {
        return None;
    }
    let ip: std::net::IpAddr = value[if detailed { "query" } else { "ip" }]
        .as_str()?
        .parse()
        .ok()?;
    let field = |key: &str| {
        value[key]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
    };
    Some(MihomoNodeCheck {
        exit_ip: Some(ip.to_string()),
        country_code: field("countryCode"),
        country_name: field("country"),
        region: field("regionName").or_else(|| field("region")),
        city: field("city"),
        ..Default::default()
    })
}

async fn diagnostic_exit(client: &reqwest::Client) -> Result<MihomoNodeCheck, AdminError> {
    // Match Sub2API's diagnostic targets. Country-filter observations remain
    // independent and still use country.is with the upstream 6-second budget.
    for (url, detailed) in [
        ("http://ip-api.com/json/?lang=zh-CN", true),
        ("http://api64.ipify.org?format=json", false),
    ] {
        let start = Instant::now();
        let result = async {
            let response = client
                .get(url)
                .timeout(Duration::from_secs(10))
                .send()
                .await
                .ok()?;
            if response.status() != reqwest::StatusCode::OK {
                return None;
            }
            parse_exit(&read_limited(response, 1024 * 1024).await.ok()?, detailed)
        }
        .await;
        if let Some(mut result) = result {
            result.latency_ms = Some(start.elapsed().as_millis() as u64);
            return Ok(result);
        }
    }
    Err(AdminError::internal("代理出口连通检测失败"))
}

pub(super) async fn check(
    dir: &Path,
    node: &Node,
    quality: bool,
    build: &super::ClientBuilder,
) -> Result<MihomoNodeCheck, AdminError> {
    tokio::time::timeout(Duration::from_secs(120), async {
        let (_guard, client) = isolated(dir, node, build).await?;
        let start = Instant::now();
        let exit = diagnostic_exit(&client).await;
        let mut result = MihomoNodeCheck {
            checked_at: Some(Utc::now()),
            latency_ms: Some(start.elapsed().as_millis() as u64),
            ..Default::default()
        };
        match exit {
            Ok(exit) => {
                result = MihomoNodeCheck {
                    checked_at: result.checked_at,
                    ..exit
                };
                result.success = Some(true);
                result.message = Some("代理出口连通正常".into());
            }
            Err(_) => {
                result.success = Some(false);
                result.message = Some("代理出口连通检测失败".into());
            }
        }
        if quality {
            let pass = result.success == Some(true);
            let mut items = vec![MihomoQualityItem {
                name: "base_connectivity".into(),
                success: pass,
                status: if pass { "pass" } else { "fail" }.into(),
                http_status: None,
                cf_ray: None,
                latency_ms: result.latency_ms.unwrap_or(0),
                reason: result.message.clone().unwrap_or_default(),
            }];
            if pass {
                for (name, url, statuses) in [
                    ("openai", "https://api.openai.com/v1/models", &[401_u16][..]),
                    (
                        "anthropic",
                        "https://api.anthropic.com/v1/messages",
                        &[401, 405, 404, 400][..],
                    ),
                    (
                        "gemini",
                        "https://generativelanguage.googleapis.com/$discovery/rest?version=v1beta",
                        &[200][..],
                    ),
                    ("grok", "https://api.x.ai/v1/models", &[401][..]),
                ] {
                    items.push(quality_target(&client, name, url, statuses).await);
                }
            }
            result.quality = Some(report(items));
        }
        Ok(result)
    })
    .await
    .map_err(|_| AdminError::internal("节点检测超时"))?
}

async fn quality_target(
    client: &reqwest::Client,
    name: &str,
    url: &str,
    allowed: &[u16],
) -> MihomoQualityItem {
    let start = Instant::now();
    let mut item = MihomoQualityItem {
        name: name.into(),
        success: false,
        status: "fail".into(),
        http_status: None,
        cf_ray: None,
        latency_ms: 0,
        reason: "请求失败".into(),
    };
    let response = client.get(url).header("Accept", "application/json,text/html,*/*")
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36")
        .send().await;
    item.latency_ms = start.elapsed().as_millis() as u64;
    let Ok(response) = response else {
        return item;
    };
    let status = response.status().as_u16();
    item.http_status = Some(status);
    item.cf_ray = response
        .headers()
        .get("cf-ray")
        .and_then(|h| h.to_str().ok())
        .filter(|s| s.len() <= 128)
        .map(str::to_owned);
    let challenge_header = response
        .headers()
        .get("cf-mitigated")
        .is_some_and(|h| h == "challenge");
    use futures::StreamExt;
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            item.reason = "读取响应失败".into();
            return item;
        };
        let remaining = 8192_usize.saturating_sub(body.len());
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if body.len() == 8192 {
            break;
        }
    }
    let body = String::from_utf8_lossy(&body).to_ascii_lowercase();
    let challenge = challenge_header
        || (matches!(status, 403 | 503)
            && (body.contains("cf-chl-")
                || body.contains("/cdn-cgi/challenge-platform/")
                || body.contains("just a moment")));
    if challenge {
        item.status = "challenge".into();
        item.reason = "命中Cloudflare challenge".into();
    } else if allowed.contains(&status) {
        item.status = "pass".into();
        item.success = true;
        item.reason = format!("HTTP {status}（目标可达）");
    } else if status == 429 {
        item.status = "warn".into();
        item.reason = "目标返回429，可能存在频控".into();
    } else {
        item.reason = format!("非预期状态码：{status}");
    }
    item
}

pub(super) fn report(checks: Vec<MihomoQualityItem>) -> MihomoQualityReport {
    let count = |status: &str| checks.iter().filter(|c| c.status == status).count();
    let passed_count = count("pass");
    let warn_count = count("warn");
    let failed_count = count("fail");
    let challenge_count = count("challenge");
    let score =
        100_usize.saturating_sub(warn_count * 10 + failed_count * 22 + challenge_count * 30) as u32;
    MihomoQualityReport {
        checked_at: Utc::now(),
        score,
        grade: match score {
            90.. => "A",
            75.. => "B",
            60.. => "C",
            40.. => "D",
            _ => "F",
        }
        .into(),
        summary: format!(
            "通过 {passed_count} 项，告警 {warn_count} 项，失败 {failed_count} 项，挑战 {challenge_count} 项"
        ),
        passed_count,
        warn_count,
        failed_count,
        challenge_count,
        checks,
    }
}
