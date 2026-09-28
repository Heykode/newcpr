use super::{
    files,
    sources::read_limited,
    state::{Saved, digest, node_id, node_name},
};
use gateway_admin::model::AdminError;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::{Child, Command};

pub(super) const VERSION: &str = "v1.19.31";
pub(super) const ENDPOINT: &str = "http://127.0.0.1:3101";
pub(super) const CONTROLLER: &str = "http://127.0.0.1:9098";
pub(super) const MAX_EXITS: usize = 4096;

pub(super) fn command(binary: &Path) -> Command {
    // The parent owns the only pipe writer. EOF also happens on SIGKILL, so
    // the guardian terminates the kernel even when Rust destructors cannot run.
    // /bin/sh is already part of CPR's supported Linux runtime image.
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(concat!(
            "exec 3<&0\n",
            "\"$@\" </dev/null &\n",
            "kernel=$!\n",
            "(while IFS= read -r line <&3; do :; done; kill -KILL \"$kernel\" 2>/dev/null) &\n",
            "guardian=$!\n",
            "wait \"$kernel\"\n",
            "result=$?\n",
            "kill -KILL \"$guardian\" 2>/dev/null\n",
            "wait \"$guardian\" 2>/dev/null\n",
            "exit \"$result\"\n",
        ))
        .arg("cpr-mihomo-child")
        .arg(binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
}

pub(super) struct Kernel {
    pub dir: PathBuf,
    child: Option<Child>,
    pub ports: BTreeMap<String, u16>,
}

impl Kernel {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            child: None,
            ports: BTreeMap::new(),
        }
    }
    pub fn supported() -> bool {
        cfg!(all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ))
    }
    pub fn installed(&self) -> bool {
        self.dir.join("mihomo").is_file()
    }
    pub fn running(&mut self) -> bool {
        let exited = self
            .child
            .as_mut()
            .is_some_and(|c| !matches!(c.try_wait(), Ok(None)));
        if exited {
            self.child = None;
        }
        self.child.is_some()
    }

    pub fn config(&mut self, saved: &Saved) -> Result<Value, AdminError> {
        let dynamic = saved.dynamic_ids();
        for node in &saved.nodes {
            let id = node_id(node);
            if !saved.eligible(node, dynamic.contains(&id)) {
                continue;
            }
            if !self.ports.contains_key(&id) {
                if self.ports.len() >= MAX_EXITS {
                    return Err(AdminError::invalid("节点独立监听端口已达到上游4096上限"));
                }
                let port = 19000
                    + u16::try_from(self.ports.len())
                        .map_err(|_| AdminError::internal("节点端口分配失败"))?;
                self.ports.insert(id, port);
            }
        }
        let active: BTreeMap<_, _> = saved
            .nodes
            .iter()
            .filter(|node| saved.eligible(node, dynamic.contains(&node_id(node))))
            .map(|node| (node_id(node), node_name(node)))
            .collect();
        let listeners: Vec<_> = self
            .ports
            .iter()
            .map(|(id, port)| {
                json!({
                    "name":format!("BPS-{port}"), "type":"mixed", "listen":"127.0.0.1", "port":port,
                    "proxy":active.get(id).copied().unwrap_or("REJECT")
                })
            })
            .collect();
        let mut names: Vec<_> = active.values().copied().collect();
        if names.is_empty() {
            names.push("REJECT");
        }
        Ok(json!({
            "mixed-port":3101,"allow-lan":false,"bind-address":"127.0.0.1","mode":"rule","log-level":"silent",
            "external-controller":"127.0.0.1:9098","secret":saved.secret,"proxies":saved.nodes,
            "proxy-groups":[{"name":"CODEX-ROTATE","type":"load-balance","strategy":"round-robin","proxies":names,"url":"https://www.gstatic.com/generate_204","interval":300}],
            "listeners":listeners,"rules":["MATCH,CODEX-ROTATE"]
        }))
    }

    pub async fn install(&self, client: &Client) -> Result<(), AdminError> {
        if !Self::supported() {
            return Err(AdminError::invalid("Mihomo受管内核支持Linux amd64/arm64"));
        }
        let arch = if cfg!(target_arch = "x86_64") {
            "amd64-compatible"
        } else {
            "arm64"
        };
        let asset = format!("mihomo-linux-{arch}-{VERSION}.gz");
        #[derive(Deserialize)]
        struct Asset {
            name: String,
            digest: Option<String>,
        }
        #[derive(Deserialize)]
        struct Release {
            assets: Vec<Asset>,
        }
        let manifest = get(
            client,
            &format!("https://api.github.com/repos/MetaCubeX/mihomo/releases/tags/{VERSION}"),
            4 << 20,
        )
        .await?;
        let release: Release = serde_json::from_slice(&manifest)
            .map_err(|_| AdminError::internal("内核发布清单无效"))?;
        let expected = release
            .assets
            .into_iter()
            .find(|a| a.name == asset)
            .and_then(|a| a.digest)
            .and_then(|s| s.strip_prefix("sha256:").map(str::to_owned))
            .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| AdminError::internal("官方内核发布缺少SHA-256校验值"))?;
        let compressed = get(
            client,
            &format!("https://github.com/MetaCubeX/mihomo/releases/download/{VERSION}/{asset}"),
            64 << 20,
        )
        .await?;
        if digest(&compressed) != expected.to_ascii_lowercase() {
            return Err(AdminError::internal("内核SHA-256校验失败"));
        }
        let mut binary = Vec::new();
        flate2::read::GzDecoder::new(compressed.as_slice())
            .take((192 << 20) + 1)
            .read_to_end(&mut binary)
            .map_err(|_| AdminError::internal("内核压缩文件无效"))?;
        if binary.len() > 192 << 20 {
            return Err(AdminError::internal("内核解压大小超限"));
        }
        files::atomic_write(&self.dir.join("mihomo"), &binary, true)
    }

    pub async fn validate(&self, config: &Value) -> Result<PathBuf, AdminError> {
        let path = self.dir.join("candidate.json");
        files::atomic_write(
            &path,
            &serde_json::to_vec(config).map_err(|_| AdminError::internal("代理配置编码失败"))?,
            false,
        )?;
        let mut child = command(&self.dir.join("mihomo"))
            .args(["-d"])
            .arg(&self.dir)
            .arg("-f")
            .arg(&path)
            .arg("-t")
            .spawn()
            .map_err(|_| AdminError::internal("无法校验内核配置"))?;
        // Child::wait closes its own stdin; retain the lifetime writer here.
        let _lifetime = child.stdin.take();
        let status = child
            .wait()
            .await
            .map_err(|_| AdminError::internal("无法校验内核配置"))?;
        if !status.success() {
            return Err(AdminError::invalid("内核拒绝候选配置，已保留原配置"));
        }
        Ok(path)
    }

    pub async fn start(
        &mut self,
        path: &Path,
        secret: &str,
        client: &Client,
    ) -> Result<(), AdminError> {
        if !Self::supported() {
            return Err(AdminError::invalid("Mihomo受管内核支持Linux amd64/arm64"));
        }
        for port in [3101, 9098].into_iter().chain(self.ports.values().copied()) {
            let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
                .await
                .map_err(|_| AdminError::invalid("Mihomo回环监听端口已被占用"))?;
            drop(listener);
        }
        self.child = Some(
            command(&self.dir.join("mihomo"))
                .arg("-d")
                .arg(&self.dir)
                .arg("-f")
                .arg(path)
                .spawn()
                .map_err(|_| AdminError::internal("无法启动Mihomo内核"))?,
        );
        for _ in 0..40 {
            if !self.running() {
                return Err(AdminError::internal("Mihomo内核启动后退出"));
            }
            if control(client, reqwest::Method::GET, "/version", secret, None)
                .await
                .is_ok()
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        self.stop().await;
        Err(AdminError::internal("Mihomo控制器启动超时"))
    }

    pub async fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            drop(child.stdin.take());
            if tokio::time::timeout(Duration::from_secs(5), child.wait())
                .await
                .is_err()
            {
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
        }
    }
}

async fn get(client: &Client, url: &str, limit: usize) -> Result<Vec<u8>, AdminError> {
    let response = client
        .get(url)
        .header("User-Agent", "CPR-Mihomo-Manager")
        .send()
        .await
        .map_err(|_| AdminError::internal("官方内核下载失败"))?;
    if !response.status().is_success() {
        return Err(AdminError::internal("官方内核下载返回非成功状态"));
    }
    read_limited(response, limit).await
}

pub(super) async fn control(
    client: &Client,
    method: reqwest::Method,
    path: &str,
    secret: &str,
    payload: Option<&Value>,
) -> Result<(), AdminError> {
    let mut request = client
        .request(method, format!("{CONTROLLER}{path}"))
        .bearer_auth(secret)
        .timeout(Duration::from_secs(2));
    if let Some(payload) = payload {
        request = request.json(payload);
    }
    let response = request
        .send()
        .await
        .map_err(|_| AdminError::internal("Mihomo控制器不可用"))?;
    if !response.status().is_success() {
        return Err(AdminError::internal("Mihomo控制器拒绝操作"));
    }
    Ok(())
}

pub(super) async fn reload(
    client: &Client,
    config: &Value,
    secret: &str,
) -> Result<(), AdminError> {
    control(
        client,
        reqwest::Method::PUT,
        "/configs?force=true",
        secret,
        Some(&json!({"payload":config.to_string()})),
    )
    .await
}
