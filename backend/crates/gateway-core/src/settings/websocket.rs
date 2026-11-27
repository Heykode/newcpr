//! OpenAI WebSocket 连接池的平台设置与边界校验

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenAiWebSocketPoolSettings {
    pub max_age_ms: u64,
    /// 保留已有实例的关闭策略；建连容量始终由账号有效并发决定。
    pub enabled: bool,
    pub fast_path_budget_ms: u64,
    pub idle_timeout_ms: u64,
    pub stream_idle_timeout_ms: u64,
}

impl Default for OpenAiWebSocketPoolSettings {
    fn default() -> Self {
        Self {
            max_age_ms: 3_300_000,
            enabled: true,
            fast_path_budget_ms: 800,
            idle_timeout_ms: 300_000,
            stream_idle_timeout_ms: 300_000,
        }
    }
}

impl OpenAiWebSocketPoolSettings {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.max_age_ms == 0 {
            return Err("openai_websocket_pool.max_age_ms");
        }
        if !(1..=15_000).contains(&self.fast_path_budget_ms) {
            return Err("openai_websocket_pool.fast_path_budget_ms");
        }
        if self.idle_timeout_ms == 0 {
            return Err("openai_websocket_pool.idle_timeout_ms");
        }
        Ok(())
    }
}
