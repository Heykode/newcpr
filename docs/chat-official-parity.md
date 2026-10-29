# Chat 官方替换记录

## 来源与范围

- 官方仓库：`zyycn/codex-proxy-rs`。
- 固定基线：`d89db84a77d0143a91770b66bec47ab8f2e57a35`，v3.35.1。
- 官方位置：`backend/crates/gateway-api/src/openai/chat_completions/{request,response}.rs`。
- 本项目位置：`backend/crates/gateway-api/src/openai/chat/{request,response}.rs`。
- 非流式汇总：同步复制官方 `responses/response.rs::collect_response` 与 `wire_response_id`，
  仅为 Chat 使用；原生 Responses 保留既有恢复入口。
- 用户明确选择官方支持范围，取代此前保留 CPA Chat 扩展的决定。
- 旧 `gateway-protocol/openai/chat` 实现与专属扩展测试已删除，不保留双路径。

## 必要接入差异

请求转换源文件原样移植。响应转换源文件只映射本项目 TokenUsage 的两个成员名：

| 官方成员 | 本项目成员 |
| --- | --- |
| `cached_input_tokens` | `cached_tokens` |
| `reasoning_output_tokens` | `reasoning_tokens` |

函数逻辑、严格校验、身份、事件分类和输出规则采用官方实现。格式由项目 rustfmt 统一。
官方 middleware/plugin 执行接口不属于本项目；外层接入现有 ExecutionSession，
使用其提交、取消、失败收尾、选号和计费流程。未引入插件系统。
因此这是官方 Chat 协议实现的替换，不是整个官方服务的二进制替换。

非流式汇总函数仅调整 Rust 可见范围，算法原样保留。官方只用同一响应中连续、
无冲突的 `output_item.done` 补齐空终态；Chat 不再调用旧的增量文本恢复函数。
终态已有内容时不拼接先前完成项；索引缺口、重复索引内容冲突或无效完成项按官方失败。

## 实际请求路径

`Chat HTTP JSON -> Responses JSON -> Generate -> 账号/身份/路由 -> WS 或 HTTP SSE`。
返回时将 Responses 结果转回 Chat JSON 或 Chat SSE。

下游 `stream` 控制交付形式；上游连接由现有 Provider 策略选择。
Chat 入口不再透传旧 `use_websocket` 扩展。直接使用原生 Responses 的传输设置不变。
转换器不接触账号指纹、代理出口、冷却、重试、额度透支或结算。

流式首包规则也按官方处理：`response.created` 可产生角色分片并提交下游，
不再沿用旧 Chat 必须等正文才提交的规则。原执行器的提交前恢复、提交后禁止重放
规则不变，但提交时间可能不同，不能声称这次协议替换对所有请求行为完全无差异。

## 交付层复查补齐

提交 `3f480c010` 后继续按同一官方基线复查，确认并修复了两处接入遗漏。
转换源文件一致不等于 HTTP 入口和交付生命周期完整，二者需要分别验证。

| 项目 | 补齐前 | 补齐后与官方来源 |
| --- | --- | --- |
| 等待首个事件 | Chat 绕过初始保活，只在输出开始后保活 | 对齐官方 `responses/keepalive.rs::response_with_encoder`，准入完成后每 15 秒保活；始终等待同一个执行任务 |
| 提交前的上游错误 | Chat 重建错误正文，丢失扩展字段和原始表示 | 删除独立重建分支，使用现有 `engine_error_response`，保留允许下发的原始正文、类型、状态和头部；容量错误仍按既有合同投影 |
| 保活之后才报错 | 先前不覆盖此路径 | 复制官方 `chat_completions/delivery.rs::chat_error_frame`，返回 Chat error SSE 和一次 DONE；保留错误扩展与 Retry-After，不发送 Responses 的失败事件 |

本项目在 `start_response` 成功之后才调用保活，复用原会话的 `ResponseCommit`；
官方由 middleware 通过 oneshot 通知准入完成。接入方式不同，准入失败不能被提前
返回的 200/心跳覆盖这一规则相同。心跳提交 HTTP 状态，不代表业务成功或提前结算。

原生 Responses 继续调用默认保活入口，错误仍编码为 `response.failed`。
未改官方 request/response 转换器、collector 或 Provider/Core/Store。

## 首次替换验证

官方 request/response 测试移植至 API 的 `chat_official` 测试目录，
仅 fixture 换为本项目原生执行器。额外验证提交失败、取消、损坏 SSE、
工具身份、终态后的事件、单次结算边界及原生 Responses/HTTP/WS 回归。
所有构建、格式检查和测试在指定远端验证环境执行。

2026-10-29，基于 `0f2f347b8dcca526a2edc8c27188c46e4d8cb73e` 加本次工作树修改，
在指定远端验证环境完成：

| 检查 | 结果 |
| --- | --- |
| API 全量 | 592 项集成测试和 2 项单元测试通过，含 45 项官方 Chat 与原生接入回归 |
| gateway-protocol | 90 项通过 |
| OpenAI transport | 516 项通过；2 项显式生成压缩/TLS 证据的测试按原标记忽略 |
| App architecture | 14 项通过 |
| `cargo check --workspace --all-targets --locked --offline` | 通过 |
| `cargo clippy -p gateway-api -p gateway-protocol --all-targets --all-features --locked --offline -- -D warnings` | 通过 |
| `cargo fmt --all --check` | 通过 |
| 官方源文件对照 | request 相同；response 仅两处字段名映射及格式；collector 仅可见范围及格式 |

检查过程修正了错误码映射牵连原生 Responses 的问题，并补齐官方非流式汇总边界；
最终测试覆盖原生错误码、原生增量恢复、原生同批次部分输出及 Chat 的官方严格规则。
原生 Provider、Core、Store、管理设置和前端未改动。

上述替换随后已提交为 `3f480c010`。

## 交付补齐验证

2026-10-29，在 `3f480c010` 加交付层补齐修改后重新验证：

| 检查 | 最终结果 |
| --- | --- |
| 新增交付专项 | 7 项通过；旧生产代码下 6 项失败、1 项准入控制通过，已复现遗漏 |
| API 全量 | 599 项集成测试和 2 项单元测试通过，含原生 Responses/WS 回归 |
| App 完整测试目标 | 后续补齐独立 PostgreSQL/Redis 配置并设置 `CI=true`，62 项全部实际执行通过，无跳过 |
| 工作区所有目标编译 | `cargo check --workspace --all-targets --locked --offline` 通过 |
| API Clippy | 所有目标和特性通过，`-D warnings` |
| 格式检查 | `cargo fmt --all --check` 通过 |
| 测试源码一致性 | 本轮 6 个交付生产/fixture 文件的本地与远端 SHA-256 一致；固定测试文件清单已登记 |

首次专项暴露了取消测试夹具默认成功收尾的问题，已显式配置取消并断言只收尾一次。
首次全量因新增测试文件未登记而失败，补登记后重新跑完整 API 套件通过。
本轮未修改协议库或 Provider，因此不将首次替换的传输测试声称为本轮重跑。

首次 App 执行漏配数据库环境，3 项账号身份用例提前返回；这属于验证遗漏。
交付补齐已提交为 `b0898a74f` 后，在指定远端建立临时隔离 PostgreSQL/Redis，
启用独立密码和 `CI=true`（缺少数据库配置直接失败），重新执行完整 App 目标。
3 项用例分别验证以下规则，均通过真实测试数据库完成，测试服务已清理：

| 用例 | 验证内容 |
| --- | --- |
| `manual_rotation_commits_same_principal_and_rejects_conflict_or_unknown_without_pg_changes` | 合法更新凭据保留设备身份；账号主体冲突或无法确认时拒绝写库 |
| `same_principal_reimport_uses_real_codec_and_keeps_the_persisted_device` | 重复导入同一账号更新凭据，保留原账号和设备身份，不新增重复账号 |
| `loopback_reauthorization_rejects_wrong_or_unknown_principals_without_pg_changes` | 模拟 OAuth 重登返回其他账号或无法确认身份时，拒绝覆盖原账号数据 |

上述用例属于账号身份与持久化集成，不是 Chat 文本、工具或流式转换功能。
补测没有修改生产代码或数据库结构，也没有接触生产数据库。

这些是协议与模拟传输验证，未使用真实上游账号发起请求，不能证明账号风控或真实
上游成功率。交付补齐已本地提交，尚未推送、发布或部署。
