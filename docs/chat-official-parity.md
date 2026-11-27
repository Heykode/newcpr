# Chat 官方接入记录

## 当前来源与边界

固定官方目标为 `db22550e6172e7a3537eb53441ae9ce73bc50cd5`。本文件描述 v3.57.0 的实际接入，旧 v3.35.1 转换器及验收记录保存在 Git 历史，不能继续作为当前拒绝字段或转换时机的要求。

`Chat HTTP -> 原始 Chat 路由 -> Provider/账号选择 -> 目标协议转换 -> 上游 -> Chat wire -> API JSON/SSE`。
Codex 和 xAI 目标是 Responses，AGY 是 GenAI。纯 Messages 转换 helper 不代表接入上游 API Key 账号或让 OAuth 支持 Messages。

## 真实调用路径与差异

| 官方职责 | NewCPR 落点与保留差异 |
| --- | --- |
| 公共 Chat 校验、工具、结果 | `gateway-protocol/src/chat_completions/`；仅适配本地 usage 字段名和格式 |
| API 入站 | `gateway-api/src/openai/chat/mod.rs`；鉴权、解压、原始模型及 session_context 进入既有 Core |
| 选号后转换 | `providers/openai/src/provider/chat.rs`、`providers/xai/src/provider/chat/`、`providers/antigravity/src/provider/chat.rs` |
| 执行和结算 | 本地 ExecutionSession/Redis 准入，保留身份、出口、严格亲和、Excel/State 与费用；没有插件执行器 |
| 交付校验 | `openai/chat/delivery.rs` 的 ChatDeliveryValidator 与 gateway-protocol 的 ChatAccumulator，经 `openai/encoding.rs` 对真实帧执行；首块、finish、usage、DONE 和尾部错误均校验 |
| 取消与保活 | 公共交付 owner 等待同一任务；15 秒初始保活不代表业务成功，断开只取消并收尾一次 |
| 原生 Responses | 保留原始交付与本地恢复合同，不使用 Chat 累加器 |

Codex 显式会话和可信补充提示在选号前以原租户命名空间参与 strict/preferred/relaxed；观测与实际选号读取同一身份。这不创建原生 response/WS 续接归属。无显式会话时仍使用原账号调度与目标请求自身的缓存提示，不推断额外持久账号绑定。

## 验证与限制

按层维护官方转换正反例、Provider 实际 loopback HTTP、API 交付/取消/结算和原生 Responses 防回退。旧 API 测试直接注入 Responses 帧的夹具已迁移：转换断言归协议/Provider，API 接收 Chat wire。没有通过删除错误断言或恢复旧转换器来通过测试。

本轮发现并用先失败、后通过回归验证了显式亲和漏接和交付校验漏接。完整结果、真实 PostgreSQL/Redis、浏览器证据与未执行的真实付费上游边界统一记录在 [v3.57.0 合并记录](upstream-v3570-native-parity.md)。模拟上游通过不证明生产账号权限或上游额度扣减。
