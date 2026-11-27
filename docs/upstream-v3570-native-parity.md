# v3.57.0 合并与补齐记录

## 固定范围

NewCPR 源码基线 `0d52db2b1597e5c068f86659e0425ba0ec65de96`（v3.55.3），官方上次基线 `88da31137b32d11015cbe0727b8b056f4476de7e`，本次目标 `db22550e6172e7a3537eb53441ae9ce73bc50cd5`。正式 v3.57.0 标签提交为 `6950c02ee460143dd63103a35a5dd679712488dc`，目标另含发行通道元数据。固定树共 430 个变更路径，重命名前后分别计数。

用户已授权采用官方原生协议边界，保留每账号 WebSocket 建连限制、普通请求并发及本地账号/出口/Excel/State 合同。插件专属运行时、上游 API Key 账号和主动互联转发排除；OAuth 池只读共享保留。本次不查询或改变生产，不推送、合并、打标签或部署。

## 执行表与实际入口

| 编号 | 最终接入、错误边界及回归落点 |
| --- | --- |
| E01 | 固定基线与完整路径表见下文；纯目录拆分按本地已有 owner 适配，不将改名当成功能。 |
| E02 | Chat 原始请求经 API→Core 路由→Provider 选号后直转目标协议。公共 Chat parser/converter 与 target-specific 错误保留；OAuth 原生 Messages、AGY Responses 与旧跨原生转换退役。旧专属 helper/测试同步清理。 |
| E03 | Codex `client_model_catalog(codex)` 过滤 Responses 能力；Anthropic 列表/详情使用 `anthropic` 目录并在分页前排序；GenAI 原始及 agent 菜单按账号权限、目录顺序、档位及分页投影。普通 OpenAI 发现保持独立。 |
| E04 | `openai/chat/delivery.rs` 的官方 ChatDeliveryValidator 与 gateway-protocol 的 ChatAccumulator 接入 `openai/encoding.rs`；首帧、终态、usage、DONE、尾部错误均走实际交付。公共 HTTP helper 经本地请求/响应 wrapper 使用；取消/结算/保活沿本地 ExecutionSession。 |
| E05 | Codex Compact 保留实际 HTTP/WS 选择；未指定 WS 的 HTTP 压缩不强制新建 WS，显式 WS、续接与原生请求分别回归。 |
| E06 | AGY `transport/request.rs` 同时供生成/计数：平台提示词前置、客户端系统部分与元数据保留。 |
| E07 | GenAI `x-goog-api-key` 为下游 Key，Authorization 优先；HTTP/JSON/SSE、独立超时、OAuth 画像和账号会话隔离接入。结构化与文本诊断均脱敏。 |
| E08 | AGY Google AI 点数、默认关闭的自动使用开关、生成资格与余额投影；列表和详情按 Provider 文案，不展示 Codex 重置卡。 |
| E09 | selected observed_plan→ProviderCallMetadata→AttemptRecord→PG usage/ops→Admin/analytics/diagnostics；代理详情与只读 pool_summary 读取 Google quota 描述。新历史优先快照，旧记录仅按 raw plan 静态回退。 |
| E10 | 保留本地下游 WS owner、重连和回收。官方中间 AGY Responses 预热已被最终协议边界移除，不恢复无消费者的 prewarm helper。oversized HTTP replay 继续用本地会话 reconnect 标记，作用于实际下一次传输。 |
| E11 | xAI 原生 wire、原始工具、动态型号、续接保存事实和 Chat 专用工具转换接入；取消旧压缩、工具改写和自动密文重试。旧 response_stored 缺失按 false。估价按实际发送模型，费用占位 0 按未报告处理，长度终止原因对齐。 |
| E12 | OpenAI 请求设置贯穿 Admin/API→PG→snapshot→前台 HTTP/WS 和后台 policy。residency 默认 us，null 清空，旧国家与 Guardian 值保留。 |
| E13 | live RequestTuningHandle 发布在线池配置；Core 初始化先发布持久值。连接寿命按创建时冻结；enabled=false 关闭复用、idle 清理/busy 结束退休。每账号有效并发限制 opening，成功/失败/取消释放，存活总数可超过 N。 |
| E14 | 0103 历史显示名、0104 运行设置与一次导入标记；首次导入旧 YAML，旧数据库 tuning 优先，之后 DB 权威。冻结旧迁移；升级计划为源码 v3.55.3→v3.57.0，恢复方式 manual。 |
| E15 | 三语言、OpenAI 请求/池卡、AGY 点数、用量思考列左对齐、备份与价格单位、多标签/长套餐保留。官方全局 ResetCreditCard 在本地为既有独立重置卡交互，不新增重复页面。 |
| E16 | 编译、格式化、测试、浏览器均在指定隔离 Linux 验证主机；真实 PG/Redis 与模拟 loopback 上游分别记录。没有用本地执行或生产流量替代。 |
| E17 | 版本说明、当前协议规范、升级计划及本地差异同批更新；保留 15 套餐、Fast/Ultra Fast、严格排队、Excel/State、身份出口、只读共享和 AGY 比较写回。 |

## 保留差异及依据

- 官方全账号共享 maxConnecting 默认 8；用户明确选择本地每账号有效并发作为 opening 上限。普通执行并发不变；不额外增加“先收回不匹配 idle 才新建”的策略。物理出口容量仍限制实际连接。
- 官方删除 YAML ws_pool/enabled=false；本地保留旧关闭策略并一次导入，避免升级悄悄开启用户关闭的连接池。之后在线配置权威，没有第二套可覆盖账号上限的 maxConnecting。
- API 路径保留本地 openai/ 与 Responses 交付布局；公共 HTTP 分类已接真实 wrapper。额外剥离 cf-/x-forwarded- 等下游代理事实及本地调度提示，避免把下游地址冒充账号身份；响应额外过滤 x-goog-api-key，防止凭据回显。
- 本地 ExecutionSession/Redis 与官方插件执行器不同。Chat 转换不持有第二份选号/租约；保留取消、提交失败、记录失败和原生 Responses 既有恢复合同。
- xAI 旧持久状态兼容是本地升级适配；缺少 response_stored 只能重放历史，不能猜测上游支持原生续接。未恢复已删除的密文/工具补偿。
- AccountSnapshot 并非本地数据模型；套餐历史直接通过 AttemptRecord 写 Store。未观测和显式未知分别处理，不能把当前账号页面标签回填成请求时事实。
- NewCPR 独立发行仓库及 verified image 部署流程保留；官方 published 通道不覆盖本仓库。源码版本文件不代表已有发布资产或部署成功。

xAI 的发送模型估价、费用占位 0 和长度终止原因属于本地遗留差异：官方上次固定基线已具备对应规则，本次通过失败回归确认后补齐，不能计为官方本轮新功能。

## 复查修复证据

此前 `82ae3b0851`、`275754ffe2` 之后继续完成接入。已复现的缺陷均保留失败和修复后证据：Chat 显式亲和、旧 xAI 会话、Google 头脱敏、Chat 交付终态、启动池配置发布、AGY 点数展示、Anthropic 菜单、xAI 计费观测及小屏价格输入数字遮挡。原始日志和一次性完整链路验收 overlay 留在仓库外，不能将 API 的临时 Provider/Store 依赖带入正式提交。

## 最终验证

以下检查均在指定隔离 Linux 验证主机执行；本地仅做编辑、Git 和隐私检查。

| 验证层 | 已完成结果 |
| --- | --- |
| 后端管理/API/Core | gateway-admin 9 + 484，gateway-api 2 + 665，gateway-core 7 + 586 全通过 |
| 协议与 Provider | gateway-protocol 194，AGY 110，OpenAI 347 + 1368，xAI 5 + 338 全通过 |
| 应用启动与架构 | gateway 应用 62 项通过，包含启动、数据库设备/重登、模块布局及依赖方向；未放宽架构检查 |
| PostgreSQL | 完整 589 项首轮 586 项通过；两条旧迁移断言修正并复测 2 项通过，备份环境补齐后复测 1 项通过，无剩余失败。覆盖请求历史、设置保存/导入、真实账单写入和备份恢复 |
| 前端 | 1195 项单测、类型/Vite 构建及全量 ESLint 通过；最后的移动布局与浏览器脚本更改另做专项检查 |
| 浏览器 | 320/390/1440 设置保存、重载、显式清空、池参数 0 和定价；Codex/AGY 点数与 15 套餐筛选；账号卡片多宽度/主题/键盘/排序/清除筛选均通过。价格遮挡有失败前与修复后证据 |
| 模拟完整 Chat 链路 | 真实 HTTP→API→Core→Codex Provider，真实 PG 账号/租约/亲和及 Redis 适配，loopback 模拟上游；JSON/SSE、function 工具、缓存用量与一次结算通过。结算使用记录型 Store，因此不把该用例称为真实账单落库验证 |

工作区全量 `cargo clippy --all-targets --all-features -- -D warnings`、Rust 格式检查通过；最终布局更改后重新构建前端通过。已按 SHA-256 比较最终 2,188 个前后端文件与验证主机源码，全部一致，退役路径无残留。

两条迁移测试保留旧数据严格比较：账单升级单独断言新增历史显示名为空；0101/0102 设置升级固定旧迁移终点并验证重复执行，再升级当前 schema 核对运行快照。备份测试在验证容器补齐同版本 PostgreSQL 客户端后通过两轮真实导出与恢复，核对新请求设置、池参数和一次导入标记；此前缺少 pg_dump 是验证环境不足。

真实付费上游、Google 重新授权及生产部署不在本次隔离验收内；不承诺零缺陷或逐字节等同官方整个服务。

## 全部 430 个路径的处置索引

以下索引列出固定树每个变更路径。`原样`指文件内容相等；`接入`指由上表实际调用链承担；`退役`指最终官方已删除的功能或旧布局由对应入口替代；`排除`只用于已授权排除项。完整服务行为不因单个文件原样而自动等价。

| 官方路径 | 树变化 | 处置 | 本地落点/依据 |
| --- | --- | --- | --- |
| `CONTRIBUTING.md` | modified | E17 · 接入 | 本地贡献规则由 AGENTS.md 与 docs/upstream-integration-checklist.md 承担；官方变更仅为外部 UI/插件仓库文档链接 |
| `README.md` | modified | E01/E15 · 接入 | README.md |
| `backend/Cargo.lock` | modified | E01/E15 · 接入 | backend/Cargo.lock |
| `backend/apps/gateway/tests/bootstrap/config.rs` | modified | E01/E15 · 接入 | backend/apps/gateway/tests/bootstrap.rs |
| `backend/crates/gateway-admin/src/model/observability.rs` | modified | E08/E09/E12 · 接入 | backend/crates/gateway-admin/src/model/observability.rs |
| `backend/crates/gateway-admin/src/model/plugins/mod.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-admin/src/model/provider_credentials.rs` | modified | E08/E09/E12 · 接入 | backend/crates/gateway-admin/src/model/provider_credentials.rs |
| `backend/crates/gateway-admin/src/model/settings.rs` | modified | E08/E09/E12 · 接入 | backend/crates/gateway-admin/src/model/settings.rs |
| `backend/crates/gateway-admin/src/ports/provider.rs` | modified | E08/E09/E12 · 接入 | backend/crates/gateway-admin/src/ports/provider.rs |
| `backend/crates/gateway-admin/src/use_case/observability/service.rs` | modified | E08/E09/E12 · 接入 | backend/crates/gateway-admin/src/use_case/observability/mod.rs |
| `backend/crates/gateway-admin/src/use_case/peer_link/upstream/projection.rs` | modified | E09 · 接入 | backend/crates/gateway-admin/src/use_case/peer_link/projection.rs |
| `backend/crates/gateway-admin/src/use_case/proxies.rs` | modified | E08/E09/E12 · 接入 | backend/crates/gateway-admin/src/use_case/proxies.rs |
| `backend/crates/gateway-admin/src/use_case/settings.rs` | modified | E08/E09/E12 · 接入 | backend/crates/gateway-admin/src/use_case/settings.rs |
| `backend/crates/gateway-admin/tests/main.rs` | modified | E08/E09/E12 · 接入 | backend/crates/gateway-admin/tests/main.rs |
| `backend/crates/gateway-admin/tests/model/provider_credentials.rs` | modified | E08/E09/E12 · 接入 | backend/crates/gateway-admin/tests/model/provider_credentials.rs |
| `backend/crates/gateway-admin/tests/model/quota_forecast/projection.rs` | modified | E08/E09/E12 · 接入 | backend/crates/gateway-admin/tests/model/quota_forecast.rs |
| `backend/crates/gateway-admin/tests/use_case/accounts/mod.rs` | modified | E08/E09/E12 · 接入 | backend/crates/gateway-admin/tests/use_case/accounts.rs |
| `backend/crates/gateway-admin/tests/use_case/freeze_recovery.rs` | modified | E08/E09/E12 · 接入 | backend/crates/providers/openai/tests/provider/contract/freeze_recovery.rs |
| `backend/crates/gateway-admin/tests/use_case/observability/mod.rs` | modified | E08/E09/E12 · 接入 | backend/crates/gateway-admin/tests/use_case/observability/mod.rs |
| `backend/crates/gateway-admin/tests/use_case/plugins/artifacts.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-admin/tests/use_case/plugins/instances/mod.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-admin/tests/use_case/proxies.rs` | modified | E08/E09/E12 · 接入 | backend/crates/gateway-admin/tests/use_case/proxies.rs |
| `backend/crates/gateway-admin/tests/use_case/service_contract.rs` | modified | E08/E09/E12 · 接入 | backend/crates/gateway-admin/tests/use_case/settings.rs |
| `backend/crates/gateway-admin/tests/use_case/settings.rs` | modified | E08/E09/E12 · 接入 | backend/crates/gateway-admin/tests/use_case/settings.rs |
| `backend/crates/gateway-api/src/admin/plugins/management.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-api/src/admin/settings.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/admin/settings.rs |
| `backend/crates/gateway-api/src/client_error.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/error.rs 与 responses/error.rs（既有错误信封） |
| `backend/crates/gateway-api/src/data_plane/antigravity.rs` | added | E04 · 原样 | backend/crates/gateway-api/src/openai/genai/agent_menu.rs |
| `backend/crates/gateway-api/src/data_plane/auth.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/auth.rs |
| `backend/crates/gateway-api/src/data_plane/body.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/request.rs 的 decompress_request_body_limit（统一解压和正文上限） |
| `backend/crates/gateway-api/src/data_plane/chat_completions/delivery.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/chat/delivery.rs |
| `backend/crates/gateway-api/src/data_plane/chat_completions/http.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/chat/http.rs |
| `backend/crates/gateway-api/src/data_plane/chat_completions/mod.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/chat/mod.rs |
| `backend/crates/gateway-api/src/data_plane/chat_completions/request.rs` | removed | E04 · 退役 | 已删除；API 入站/错误/交付及原生回归 |
| `backend/crates/gateway-api/src/data_plane/chat_completions/response.rs` | removed | E04 · 退役 | 已删除；API 入站/错误/交付及原生回归 |
| `backend/crates/gateway-api/src/data_plane/codex/catalog.rs` | added | E03 · 接入 | backend/crates/gateway-api/src/openai/models.rs |
| `backend/crates/gateway-api/src/data_plane/codex/mod.rs` | added | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/request.rs |
| `backend/crates/gateway-api/src/data_plane/codex/request.rs` | added | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/request.rs |
| `backend/crates/gateway-api/src/data_plane/error.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/error.rs |
| `backend/crates/gateway-api/src/data_plane/genai/mod.rs` | modified | E03 · 接入 | backend/crates/gateway-api/src/openai/genai/mod.rs |
| `backend/crates/gateway-api/src/data_plane/genai/models.rs` | modified | E03 · 接入 | backend/crates/gateway-api/src/openai/genai/models.rs |
| `backend/crates/gateway-api/src/data_plane/generation/http.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/http.rs |
| `backend/crates/gateway-api/src/data_plane/generation/keepalive.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/keepalive.rs |
| `backend/crates/gateway-api/src/data_plane/generation/mod.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/mod.rs |
| `backend/crates/gateway-api/src/data_plane/generation/request.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/request.rs |
| `backend/crates/gateway-api/src/data_plane/generation/response.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/response.rs |
| `backend/crates/gateway-api/src/data_plane/generation/validation.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/request.rs（原生与 Chat 分别校验） |
| `backend/crates/gateway-api/src/data_plane/headers.rs` | added | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/request.rs |
| `backend/crates/gateway-api/src/data_plane/images/http.rs` | removed | E04 · 退役 | 已删除；API 入站/错误/交付及原生回归 |
| `backend/crates/gateway-api/src/data_plane/images/mod.rs` | removed | E04 · 退役 | 已删除；API 入站/错误/交付及原生回归 |
| `backend/crates/gateway-api/src/data_plane/images/request.rs` | removed | E04 · 退役 | 已删除；API 入站/错误/交付及原生回归 |
| `backend/crates/gateway-api/src/data_plane/live/http.rs` | removed | E04 · 退役 | 已删除；API 入站/错误/交付及原生回归 |
| `backend/crates/gateway-api/src/data_plane/live/mod.rs` | removed | E04 · 退役 | 已删除；API 入站/错误/交付及原生回归 |
| `backend/crates/gateway-api/src/data_plane/live/websocket.rs` | removed | E04 · 退役 | 已删除；API 入站/错误/交付及原生回归 |
| `backend/crates/gateway-api/src/data_plane/messages.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/messages.rs |
| `backend/crates/gateway-api/src/data_plane/mod.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/mod.rs |
| `backend/crates/gateway-api/src/data_plane/models.rs` | modified | E03 · 接入 | backend/crates/gateway-api/src/openai/models.rs |
| `backend/crates/gateway-api/src/data_plane/openai/endpoints.rs` | added | E04 · 接入 | backend/crates/gateway-api/src/openai/endpoint.rs |
| `backend/crates/gateway-api/src/data_plane/openai/images/http.rs` | added | E04 · 接入 | backend/crates/gateway-api/src/openai/images/http.rs |
| `backend/crates/gateway-api/src/data_plane/openai/images/mod.rs` | added | E04 · 接入 | backend/crates/gateway-api/src/openai/images/mod.rs |
| `backend/crates/gateway-api/src/data_plane/openai/images/request.rs` | added | E04 · 接入 | backend/crates/gateway-api/src/openai/images/http.rs 与 multipart.rs |
| `backend/crates/gateway-api/src/data_plane/openai/live/http.rs` | added | E04 · 接入 | backend/crates/gateway-api/src/openai/live/http.rs |
| `backend/crates/gateway-api/src/data_plane/openai/live/mod.rs` | added | E04 · 接入 | backend/crates/gateway-api/src/openai/live/mod.rs |
| `backend/crates/gateway-api/src/data_plane/openai/live/websocket.rs` | added | E04 · 接入 | backend/crates/gateway-api/src/openai/live/websocket.rs |
| `backend/crates/gateway-api/src/data_plane/openai/mod.rs` | added | E04 · 接入 | backend/crates/gateway-api/src/openai/mod.rs |
| `backend/crates/gateway-api/src/data_plane/openai/search/http.rs` | added | E04 · 接入 | backend/crates/gateway-api/src/openai/search/http.rs |
| `backend/crates/gateway-api/src/data_plane/openai/search/mod.rs` | added | E04 · 接入 | backend/crates/gateway-api/src/openai/search/mod.rs |
| `backend/crates/gateway-api/src/data_plane/openai_endpoints.rs` | removed | E04 · 退役 | 已删除；API 入站/错误/交付及原生回归 |
| `backend/crates/gateway-api/src/data_plane/peer_link/mod.rs` | modified | E09 · 接入 | backend/crates/gateway-api/src/openai/peer_link/mod.rs |
| `backend/crates/gateway-api/src/data_plane/responses/http.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/http.rs |
| `backend/crates/gateway-api/src/data_plane/responses/mod.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/mod.rs |
| `backend/crates/gateway-api/src/data_plane/responses/request.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/request.rs |
| `backend/crates/gateway-api/src/data_plane/responses/response.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/response.rs |
| `backend/crates/gateway-api/src/data_plane/responses/validation.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/request.rs |
| `backend/crates/gateway-api/src/data_plane/responses/websocket/connection/frame.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/websocket/connection/mod.rs 与 responses/websocket/protocol.rs（原生帧及绑定） |
| `backend/crates/gateway-api/src/data_plane/responses/websocket/connection/handle.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/websocket/connection/mod.rs（本地下游 owner） |
| `backend/crates/gateway-api/src/data_plane/responses/websocket/connection/middleware.rs` | modified | X1 · 排除 | 仅插件转换钩子及 error_code 上下文变更；本地无插件中间件，原生交付沿 connection/mod.rs |
| `backend/crates/gateway-api/src/data_plane/responses/websocket/connection/pump.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/websocket/connection/transport.rs（实际读写循环） |
| `backend/crates/gateway-api/src/data_plane/responses/websocket/forward.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/websocket/forward.rs |
| `backend/crates/gateway-api/src/data_plane/responses/websocket/mod.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/websocket/mod.rs |
| `backend/crates/gateway-api/src/data_plane/responses/websocket/protocol.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/websocket/protocol.rs |
| `backend/crates/gateway-api/src/data_plane/router.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/router.rs |
| `backend/crates/gateway-api/src/data_plane/search/http.rs` | removed | E04 · 退役 | 已删除；API 入站/错误/交付及原生回归 |
| `backend/crates/gateway-api/src/data_plane/search/mod.rs` | removed | E04 · 退役 | 已删除；API 入站/错误/交付及原生回归 |
| `backend/crates/gateway-api/src/data_plane/service.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/openai/service.rs |
| `backend/crates/gateway-api/src/data_plane/usage.rs` | modified | E04 · 接入 | 本次只将官方 usage 入口鉴权显式限定 Bearer；本地未引入该独立入口。GenAI 以外入口继续使用 openai/auth.rs 的 Bearer 鉴权 |
| `backend/crates/gateway-api/src/lib.rs` | modified | E04 · 接入 | backend/crates/gateway-api/src/lib.rs |
| `backend/crates/gateway-api/src/middleware/websocket.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-api/tests/admin/accounts/handlers.rs` | modified | E04 · 接入 | backend/crates/gateway-api/tests/admin/accounts/handlers.rs |
| `backend/crates/gateway-api/tests/admin/mod.rs` | modified | E04 · 接入 | backend/crates/gateway-api/tests/admin/mod.rs |
| `backend/crates/gateway-api/tests/admin/observability/response.rs` | modified | E04 · 接入 | backend/crates/gateway-api/tests/admin/observability/response.rs |
| `backend/crates/gateway-api/tests/admin/plugins/mod.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-api/tests/admin/settings.rs` | modified | E04 · 接入 | backend/crates/gateway-api/tests/admin/settings.rs |
| `backend/crates/gateway-api/tests/data_plane/chat_completions/delivery.rs` | added | E04 · 接入 | backend/crates/gateway-api/tests/openai/responses/chat_official/delivery.rs |
| `backend/crates/gateway-api/tests/data_plane/chat_completions/http.rs` | modified | E04 · 接入 | backend/crates/gateway-api/tests/openai/responses/chat_official/http.rs |
| `backend/crates/gateway-api/tests/data_plane/chat_completions/mod.rs` | modified | E04 · 接入 | backend/crates/gateway-api/tests/openai/responses/chat_official/mod.rs |
| `backend/crates/gateway-api/tests/data_plane/chat_completions/request.rs` | removed | E04 · 退役 | 已删除；API 入站/错误/交付及原生回归 |
| `backend/crates/gateway-api/tests/data_plane/chat_completions/response.rs` | removed | E04 · 退役 | 已删除；API 入站/错误/交付及原生回归 |
| `backend/crates/gateway-api/tests/data_plane/error.rs` | modified | E04 · 接入 | backend/crates/gateway-api/tests/openai/error.rs |
| `backend/crates/gateway-api/tests/data_plane/genai/contract.rs` | modified | E03 · 接入 | backend/crates/gateway-api/tests/openai/genai/contract.rs |
| `backend/crates/gateway-api/tests/data_plane/genai/models.rs` | modified | E03 · 接入 | backend/crates/gateway-api/tests/openai/genai/models.rs |
| `backend/crates/gateway-api/tests/data_plane/images/mod.rs` | removed | E04 · 退役 | 已删除；API 入站/错误/交付及原生回归 |
| `backend/crates/gateway-api/tests/data_plane/images/request.rs` | removed | E04 · 退役 | 已删除；API 入站/错误/交付及原生回归 |
| `backend/crates/gateway-api/tests/data_plane/live/mod.rs` | removed | E04 · 退役 | 已删除；API 入站/错误/交付及原生回归 |
| `backend/crates/gateway-api/tests/data_plane/live/websocket.rs` | removed | E04 · 退役 | 已删除；API 入站/错误/交付及原生回归 |
| `backend/crates/gateway-api/tests/data_plane/mod.rs` | modified | E04 · 接入 | backend/crates/gateway-api/tests/openai/mod.rs |
| `backend/crates/gateway-api/tests/data_plane/models.rs` | modified | E03 · 接入 | backend/crates/gateway-api/tests/openai/models.rs |
| `backend/crates/gateway-api/tests/data_plane/openai/endpoints.rs` | added | E04 · 接入 | backend/crates/gateway-api/tests/openai/endpoint.rs |
| `backend/crates/gateway-api/tests/data_plane/openai/images/mod.rs` | added | E04 · 接入 | backend/crates/gateway-api/tests/openai/images/mod.rs |
| `backend/crates/gateway-api/tests/data_plane/openai/images/request.rs` | added | E04 · 接入 | backend/crates/gateway-api/tests/openai/images/mod.rs 与 multipart.rs |
| `backend/crates/gateway-api/tests/data_plane/openai/live/mod.rs` | added | E04 · 接入 | backend/crates/gateway-api/tests/openai/live/mod.rs |
| `backend/crates/gateway-api/tests/data_plane/openai/live/websocket.rs` | added | E04 · 接入 | backend/crates/gateway-api/tests/openai/live/websocket.rs |
| `backend/crates/gateway-api/tests/data_plane/openai/mod.rs` | added | E04 · 接入 | backend/crates/gateway-api/tests/openai/mod.rs |
| `backend/crates/gateway-api/tests/data_plane/openai/search/http.rs` | added | E04 · 接入 | backend/crates/gateway-api/tests/openai/search/mod.rs |
| `backend/crates/gateway-api/tests/data_plane/openai/search/mod.rs` | added | E04 · 接入 | backend/crates/gateway-api/tests/openai/search/mod.rs |
| `backend/crates/gateway-api/tests/data_plane/openai_endpoints.rs` | removed | E04 · 退役 | 已删除；API 入站/错误/交付及原生回归 |
| `backend/crates/gateway-api/tests/data_plane/peer_link/mod.rs` | modified | E09 · 接入 | backend/crates/gateway-api/tests/openai/peer_link/mod.rs |
| `backend/crates/gateway-api/tests/data_plane/responses/http.rs` | modified | E04 · 接入 | backend/crates/gateway-api/tests/openai/responses/http.rs |
| `backend/crates/gateway-api/tests/data_plane/responses/mod.rs` | modified | E04 · 接入 | backend/crates/gateway-api/tests/openai/responses/mod.rs |
| `backend/crates/gateway-api/tests/data_plane/responses/websocket/connection/mod.rs` | modified | E04 · 接入 | backend/crates/gateway-api/tests/openai/responses/websocket/connection/mod.rs |
| `backend/crates/gateway-api/tests/data_plane/responses/websocket/forward.rs` | modified | E04 · 接入 | backend/crates/gateway-api/tests/openai/responses/websocket/forward.rs |
| `backend/crates/gateway-api/tests/data_plane/responses/websocket/interrupt.rs` | modified | E04 · 接入 | backend/crates/gateway-api/tests/openai/responses/websocket/interrupt.rs |
| `backend/crates/gateway-api/tests/data_plane/responses/websocket/mod.rs` | modified | E04 · 接入 | backend/crates/gateway-api/tests/openai/responses/websocket/mod.rs |
| `backend/crates/gateway-api/tests/data_plane/router.rs` | modified | E04 · 接入 | backend/crates/gateway-api/tests/openai/mod.rs |
| `backend/crates/gateway-api/tests/data_plane/search/http.rs` | removed | E04 · 退役 | 已删除；API 入站/错误/交付及原生回归 |
| `backend/crates/gateway-api/tests/data_plane/search/mod.rs` | removed | E04 · 退役 | 已删除；API 入站/错误/交付及原生回归 |
| `backend/crates/gateway-core/src/account/location.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/src/account/location.rs |
| `backend/crates/gateway-core/src/account/mod.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/src/account/mod.rs |
| `backend/crates/gateway-core/src/account/model.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/src/account/mod.rs |
| `backend/crates/gateway-core/src/engine/coordinator.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/src/engine/coordinator.rs |
| `backend/crates/gateway-core/src/engine/execution/contract.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/src/engine/execution.rs |
| `backend/crates/gateway-core/src/engine/execution/service.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/src/engine/execution.rs |
| `backend/crates/gateway-core/src/engine/middleware/plan.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-core/src/engine/middleware/websocket.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-core/src/engine/mod.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/src/engine/mod.rs |
| `backend/crates/gateway-core/src/engine/provider.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/src/engine/provider.rs |
| `backend/crates/gateway-core/src/error/mod.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/src/error/mod.rs |
| `backend/crates/gateway-core/src/event.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/src/event.rs |
| `backend/crates/gateway-core/src/operation.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/src/operation/mod.rs |
| `backend/crates/gateway-core/src/provider_ports/runtime_policy.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/src/provider_ports/mod.rs |
| `backend/crates/gateway-core/src/routing/catalog.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/src/routing/catalog.rs |
| `backend/crates/gateway-core/src/routing/mod.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/src/routing/mod.rs |
| `backend/crates/gateway-core/src/routing/snapshot.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/src/routing/snapshot.rs |
| `backend/crates/gateway-core/src/settings/compiled.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/src/routing/snapshot.rs |
| `backend/crates/gateway-core/src/settings/mod.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/src/settings/mod.rs |
| `backend/crates/gateway-core/src/settings/openai.rs` | added | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/src/settings/openai.rs |
| `backend/crates/gateway-core/src/settings/values.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/src/routing/snapshot.rs |
| `backend/crates/gateway-core/src/settings/websocket.rs` | added | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/src/settings/websocket.rs |
| `backend/crates/gateway-core/tests/engine/execution/identity.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/tests/engine/execution.rs |
| `backend/crates/gateway-core/tests/engine/execution/mod.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/tests/engine/execution.rs |
| `backend/crates/gateway-core/tests/error/contract.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/tests/error/mod.rs |
| `backend/crates/gateway-core/tests/operation.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/tests/operation/mod.rs |
| `backend/crates/gateway-core/tests/routing/mod.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/tests/routing/mod.rs |
| `backend/crates/gateway-core/tests/routing/snapshot.rs` | modified | E02/E10/E12/E13 · 接入 | backend/crates/gateway-core/tests/routing/snapshot.rs |
| `backend/crates/gateway-plugin/runtime/src/adapter/catalog.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/runtime/src/adapter/middleware/execution.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/runtime/src/adapter/middleware/mod.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/runtime/src/adapter/middleware/request.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/runtime/src/callback/middleware/mod.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/runtime/src/callback/model.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/runtime/src/generation/instance.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/runtime/src/generation/snapshot.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/runtime/src/package/inspection.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/runtime/tests/generation/data.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/runtime/tests/generation/policy/mounts.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/runtime/tests/generation/policy/service/contract.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/runtime/tests/generation/policy/websocket/mod.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/runtime/tests/generation/snapshot.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/runtime/tests/support/middleware/mod.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/runtime/tests/support/mod.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/sdk/README.md` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/sdk/docs/capabilities.md` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/sdk/docs/manifest.md` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/sdk/src/call/middleware/mod.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/sdk/src/call/services/settings.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/sdk/src/capability.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/sdk/src/client/middleware/plugin.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/sdk/src/manifest.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/sdk/tests/call/middleware/mod.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/sdk/tests/client/session/mod.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-plugin/sdk/tests/manifest.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-protocol/Cargo.toml` | modified | E02/E04 · 接入 | backend/crates/gateway-protocol/Cargo.toml |
| `backend/crates/gateway-protocol/src/anthropic/content.rs` | removed | E02/E04 · 退役 | 已删除；共享协议、原生解析与 Chat 转换；移除旧跨原生转换 |
| `backend/crates/gateway-protocol/src/anthropic/error.rs` | modified | E02/E04 · 原样 | backend/crates/gateway-protocol/src/anthropic/error.rs |
| `backend/crates/gateway-protocol/src/anthropic/mod.rs` | modified | E02/E04 · 原样 | backend/crates/gateway-protocol/src/anthropic/mod.rs |
| `backend/crates/gateway-protocol/src/anthropic/request.rs` | modified | E02/E04 · 原样 | backend/crates/gateway-protocol/src/anthropic/request.rs |
| `backend/crates/gateway-protocol/src/anthropic/response.rs` | removed | E02/E04 · 退役 | 已删除；共享协议、原生解析与 Chat 转换；移除旧跨原生转换 |
| `backend/crates/gateway-protocol/src/anthropic/stream/accumulator.rs` | modified | E02/E04 · 原样 | backend/crates/gateway-protocol/src/anthropic/stream/accumulator.rs |
| `backend/crates/gateway-protocol/src/anthropic/stream/mod.rs` | modified | E02/E04 · 原样 | backend/crates/gateway-protocol/src/anthropic/stream/mod.rs |
| `backend/crates/gateway-protocol/src/anthropic/stream/to_messages.rs` | removed | E02/E04 · 退役 | 已删除；共享协议、原生解析与 Chat 转换；移除旧跨原生转换 |
| `backend/crates/gateway-protocol/src/anthropic/stream/to_responses.rs` | removed | E02/E04 · 退役 | 已删除；共享协议、原生解析与 Chat 转换；移除旧跨原生转换 |
| `backend/crates/gateway-protocol/src/anthropic/thinking.rs` | modified | E02/E04 · 原样 | backend/crates/gateway-protocol/src/anthropic/thinking.rs |
| `backend/crates/gateway-protocol/src/anthropic/tools.rs` | removed | E02/E04 · 退役 | 已删除；共享协议、原生解析与 Chat 转换；移除旧跨原生转换 |
| `backend/crates/gateway-protocol/src/anthropic/usage.rs` | modified | E02/E04 · 原样 | backend/crates/gateway-protocol/src/anthropic/usage.rs |
| `backend/crates/gateway-protocol/src/chat_completions/error.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/src/chat_completions/error.rs |
| `backend/crates/gateway-protocol/src/chat_completions/genai/mod.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/src/chat_completions/genai/mod.rs |
| `backend/crates/gateway-protocol/src/chat_completions/genai/request.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/src/chat_completions/genai/request.rs |
| `backend/crates/gateway-protocol/src/chat_completions/genai/response.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/src/chat_completions/genai/response.rs |
| `backend/crates/gateway-protocol/src/chat_completions/genai/stream.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/src/chat_completions/genai/stream.rs |
| `backend/crates/gateway-protocol/src/chat_completions/messages/mod.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/src/chat_completions/messages/mod.rs |
| `backend/crates/gateway-protocol/src/chat_completions/messages/request.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/src/chat_completions/messages/request.rs |
| `backend/crates/gateway-protocol/src/chat_completions/messages/response.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/src/chat_completions/messages/response.rs |
| `backend/crates/gateway-protocol/src/chat_completions/messages/stream.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/src/chat_completions/messages/stream.rs |
| `backend/crates/gateway-protocol/src/chat_completions/mod.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/src/chat_completions/mod.rs |
| `backend/crates/gateway-protocol/src/chat_completions/request.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/src/chat_completions/request.rs |
| `backend/crates/gateway-protocol/src/chat_completions/response.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/src/chat_completions/response.rs |
| `backend/crates/gateway-protocol/src/chat_completions/responses/mod.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/src/chat_completions/responses/mod.rs |
| `backend/crates/gateway-protocol/src/chat_completions/responses/request.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/src/chat_completions/responses/request.rs |
| `backend/crates/gateway-protocol/src/chat_completions/responses/response.rs` | added | E02/E04 · 接入 | backend/crates/gateway-protocol/src/chat_completions/responses/response.rs |
| `backend/crates/gateway-protocol/src/genai/grounding.rs` | removed | E02/E04 · 退役 | 已删除；共享协议、原生解析与 Chat 转换；移除旧跨原生转换 |
| `backend/crates/gateway-protocol/src/genai/messages.rs` | removed | E02/E04 · 退役 | 已删除；共享协议、原生解析与 Chat 转换；移除旧跨原生转换 |
| `backend/crates/gateway-protocol/src/genai/mod.rs` | modified | E02/E04 · 原样 | backend/crates/gateway-protocol/src/genai/mod.rs |
| `backend/crates/gateway-protocol/src/genai/responses.rs` | removed | E02/E04 · 退役 | 已删除；共享协议、原生解析与 Chat 转换；移除旧跨原生转换 |
| `backend/crates/gateway-protocol/src/genai/stream.rs` | removed | E02/E04 · 退役 | 已删除；共享协议、原生解析与 Chat 转换；移除旧跨原生转换 |
| `backend/crates/gateway-protocol/src/genai/tokens.rs` | modified | E02/E04 · 原样 | backend/crates/gateway-protocol/src/genai/tokens.rs |
| `backend/crates/gateway-protocol/src/http.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/src/http.rs |
| `backend/crates/gateway-protocol/src/lib.rs` | modified | E02/E04 · 接入 | backend/crates/gateway-protocol/src/lib.rs |
| `backend/crates/gateway-protocol/src/openai/events.rs` | modified | E02/E04 · 接入 | backend/crates/gateway-protocol/src/openai/events.rs |
| `backend/crates/gateway-protocol/src/openai/headers.rs` | modified | E02/E04 · 接入 | backend/crates/gateway-protocol/src/openai/headers.rs |
| `backend/crates/gateway-protocol/src/openai/mod.rs` | modified | E02/E04 · 接入 | backend/crates/gateway-protocol/src/openai/mod.rs |
| `backend/crates/gateway-protocol/src/openai/sse.rs` | modified | E02/E04 · 接入 | backend/crates/gateway-protocol/src/openai/sse.rs |
| `backend/crates/gateway-protocol/src/openai/web_search.rs` | modified | E02/E04 · 接入 | backend/crates/gateway-protocol/src/openai/web_search.rs |
| `backend/crates/gateway-protocol/src/request.rs` | modified | E02/E04 · 接入 | backend/crates/gateway-protocol/src/request.rs |
| `backend/crates/gateway-protocol/tests/anthropic/mod.rs` | modified | E02/E04 · 原样 | backend/crates/gateway-protocol/tests/anthropic/mod.rs |
| `backend/crates/gateway-protocol/tests/anthropic/request.rs` | modified | E02/E04 · 原样 | backend/crates/gateway-protocol/tests/anthropic/request.rs |
| `backend/crates/gateway-protocol/tests/anthropic/response.rs` | removed | E02/E04 · 退役 | 已删除；共享协议、原生解析与 Chat 转换；移除旧跨原生转换 |
| `backend/crates/gateway-protocol/tests/anthropic/stream/mod.rs` | modified | E02/E04 · 原样 | backend/crates/gateway-protocol/tests/anthropic/stream/mod.rs |
| `backend/crates/gateway-protocol/tests/anthropic/stream/translation.rs` | removed | E02/E04 · 退役 | 已删除；共享协议、原生解析与 Chat 转换；移除旧跨原生转换 |
| `backend/crates/gateway-protocol/tests/anthropic/usage.rs` | modified | E02/E04 · 原样 | backend/crates/gateway-protocol/tests/anthropic/usage.rs |
| `backend/crates/gateway-protocol/tests/chat_completions/genai/mod.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/tests/chat_completions/genai/mod.rs |
| `backend/crates/gateway-protocol/tests/chat_completions/genai/request.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/tests/chat_completions/genai/request.rs |
| `backend/crates/gateway-protocol/tests/chat_completions/genai/response.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/tests/chat_completions/genai/response.rs |
| `backend/crates/gateway-protocol/tests/chat_completions/genai/stream.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/tests/chat_completions/genai/stream.rs |
| `backend/crates/gateway-protocol/tests/chat_completions/messages/mod.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/tests/chat_completions/messages/mod.rs |
| `backend/crates/gateway-protocol/tests/chat_completions/messages/request.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/tests/chat_completions/messages/request.rs |
| `backend/crates/gateway-protocol/tests/chat_completions/messages/response.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/tests/chat_completions/messages/response.rs |
| `backend/crates/gateway-protocol/tests/chat_completions/messages/stream.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/tests/chat_completions/messages/stream.rs |
| `backend/crates/gateway-protocol/tests/chat_completions/mod.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/tests/chat_completions/mod.rs |
| `backend/crates/gateway-protocol/tests/chat_completions/request.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/tests/chat_completions/request.rs |
| `backend/crates/gateway-protocol/tests/chat_completions/response.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/tests/chat_completions/response.rs |
| `backend/crates/gateway-protocol/tests/chat_completions/responses/mod.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/tests/chat_completions/responses/mod.rs |
| `backend/crates/gateway-protocol/tests/chat_completions/responses/request.rs` | added | E02/E04 · 接入 | backend/crates/gateway-protocol/tests/chat_completions/responses/request.rs |
| `backend/crates/gateway-protocol/tests/chat_completions/responses/response.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/tests/chat_completions/responses/response.rs |
| `backend/crates/gateway-protocol/tests/genai/messages.rs` | removed | E02/E04 · 退役 | 已删除；共享协议、原生解析与 Chat 转换；移除旧跨原生转换 |
| `backend/crates/gateway-protocol/tests/genai/mod.rs` | modified | E02/E04 · 原样 | backend/crates/gateway-protocol/tests/genai/mod.rs |
| `backend/crates/gateway-protocol/tests/genai/responses.rs` | removed | E02/E04 · 退役 | 已删除；共享协议、原生解析与 Chat 转换；移除旧跨原生转换 |
| `backend/crates/gateway-protocol/tests/genai/tokens.rs` | modified | E02/E04 · 原样 | backend/crates/gateway-protocol/tests/genai/tokens.rs |
| `backend/crates/gateway-protocol/tests/http.rs` | added | E02/E04 · 原样 | backend/crates/gateway-protocol/tests/http.rs |
| `backend/crates/gateway-protocol/tests/main.rs` | modified | E02/E04 · 接入 | backend/crates/gateway-protocol/tests/main.rs |
| `backend/crates/gateway-protocol/tests/openai/headers.rs` | modified | E02/E04 · 接入 | backend/crates/gateway-protocol/tests/openai/headers.rs |
| `backend/crates/gateway-protocol/tests/openai/sse.rs` | modified | E02/E04 · 接入 | backend/crates/gateway-protocol/tests/openai/sse.rs |
| `backend/crates/gateway-protocol/tests/openai/web_search.rs` | modified | E02/E04 · 接入 | backend/crates/gateway-protocol/tests/openai/web_search.rs |
| `backend/crates/gateway-store/src/admin_adapter.rs` | modified | E09/E14 · 接入 | backend/crates/gateway-store/src/admin_adapter.rs |
| `backend/crates/gateway-store/src/postgres/execution/mod.rs` | modified | E09/E14 · 接入 | backend/crates/gateway-store/src/postgres/execution.rs |
| `backend/crates/gateway-store/src/postgres/execution_buffer.rs` | modified | E09/E14 · 接入 | backend/crates/gateway-store/src/postgres/execution_buffer.rs |
| `backend/crates/gateway-store/src/postgres/observability/analytics/query.rs` | modified | E09/E14 · 接入 | backend/crates/gateway-store/src/postgres/observability/analytics/query.rs |
| `backend/crates/gateway-store/src/postgres/observability/mapping.rs` | modified | E09/E14 · 接入 | backend/crates/gateway-store/src/postgres/observability/mapping.rs |
| `backend/crates/gateway-store/src/postgres/observability/model.rs` | modified | E09/E14 · 接入 | backend/crates/gateway-store/src/postgres/observability/model.rs |
| `backend/crates/gateway-store/src/postgres/observability/queries/ops.rs` | modified | E09/E14 · 接入 | backend/crates/gateway-store/src/postgres/observability/queries/ops.rs |
| `backend/crates/gateway-store/src/postgres/observability/queries/usage.rs` | modified | E09/E14 · 接入 | backend/crates/gateway-store/src/postgres/observability/queries/usage.rs |
| `backend/crates/gateway-store/src/postgres/ops_events.rs` | modified | E09/E14 · 接入 | backend/crates/gateway-store/src/postgres/ops_events.rs |
| `backend/crates/gateway-store/src/postgres/runtime_settings.rs` | modified | E09/E14 · 接入 | backend/crates/gateway-store/src/postgres/runtime_settings.rs |
| `backend/crates/gateway-store/src/postgres/snapshot.rs` | modified | E09/E14 · 接入 | backend/crates/gateway-store/src/postgres/snapshot.rs |
| `backend/crates/gateway-store/src/postgres/usage_facts.rs` | modified | E09/E14 · 接入 | backend/crates/gateway-store/src/postgres/execution.rs |
| `backend/crates/gateway-store/tests/postgres/control_plane.rs` | modified | E09/E14 · 接入 | backend/crates/gateway-store/tests/postgres/runtime_settings.rs |
| `backend/crates/gateway-store/tests/postgres/execution/mod.rs` | modified | E09/E14 · 接入 | backend/crates/gateway-store/tests/postgres/execution.rs |
| `backend/crates/gateway-store/tests/postgres/plugins/artifacts.rs` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `backend/crates/gateway-store/tests/postgres/runtime_settings.rs` | modified | E09/E14 · 接入 | backend/crates/gateway-store/tests/postgres/runtime_settings.rs |
| `backend/crates/gateway-store/tests/postgres/snapshots.rs` | modified | E09/E14 · 接入 | backend/crates/gateway-store/tests/postgres/snapshots.rs |
| `backend/crates/gateway-store/tests/postgres/usage_data/read.rs` | modified | E09/E14 · 接入 | backend/crates/gateway-store/tests/postgres/observability/usage_history.rs |
| `backend/crates/providers/antigravity/src/admin/provider.rs` | modified | E06/E07/E08 · 接入 | backend/crates/providers/antigravity/src/admin/provider.rs |
| `backend/crates/providers/antigravity/src/credential/catalog.rs` | modified | E06/E07/E08 · 接入 | backend/crates/providers/antigravity/src/credential/catalog.rs |
| `backend/crates/providers/antigravity/src/credential/model_groups.rs` | modified | E06/E07/E08 · 接入 | backend/crates/providers/antigravity/src/credential/model_groups.rs |
| `backend/crates/providers/antigravity/src/credential/quota/document.rs` | modified | E06/E07/E08 · 接入 | backend/crates/providers/antigravity/src/credential/quota/document.rs |
| `backend/crates/providers/antigravity/src/credential/quota/mod.rs` | modified | E06/E07/E08 · 原样 | backend/crates/providers/antigravity/src/credential/quota/mod.rs |
| `backend/crates/providers/antigravity/src/credential/quota/service.rs` | modified | E06/E07/E08 · 接入 | backend/crates/providers/antigravity/src/credential/quota/service.rs |
| `backend/crates/providers/antigravity/src/profile/selection.rs` | modified | E06/E07/E08 · 原样 | backend/crates/providers/antigravity/src/profile/selection.rs |
| `backend/crates/providers/antigravity/src/provider/affinity.rs` | modified | E06/E07/E08 · 原样 | backend/crates/providers/antigravity/src/provider/affinity.rs |
| `backend/crates/providers/antigravity/src/provider/candidates.rs` | modified | E06/E07/E08 · 接入 | backend/crates/providers/antigravity/src/provider/candidates.rs |
| `backend/crates/providers/antigravity/src/provider/chat.rs` | added | E06/E07/E08 · 原样 | backend/crates/providers/antigravity/src/provider/chat.rs |
| `backend/crates/providers/antigravity/src/provider/count.rs` | added | E06/E07/E08 · 原样 | backend/crates/providers/antigravity/src/provider/count.rs |
| `backend/crates/providers/antigravity/src/provider/execution.rs` | modified | E06/E07/E08 · 接入 | backend/crates/providers/antigravity/src/provider/execution.rs |
| `backend/crates/providers/antigravity/src/provider/mod.rs` | modified | E06/E07/E08 · 接入 | backend/crates/providers/antigravity/src/provider/mod.rs |
| `backend/crates/providers/antigravity/src/provider/selection.rs` | modified | E06/E07/E08 · 接入 | backend/crates/providers/antigravity/src/provider/selection.rs |
| `backend/crates/providers/antigravity/src/provider/translation.rs` | removed | E06/E07/E08 · 退役 | 已删除；GenAI/Chat、系统指令、账号/点数与传输 |
| `backend/crates/providers/antigravity/src/provider/web_search.rs` | removed | E06/E07/E08 · 退役 | 已删除；GenAI/Chat、系统指令、账号/点数与传输 |
| `backend/crates/providers/antigravity/src/transport/account.rs` | modified | E06/E07/E08 · 原样 | backend/crates/providers/antigravity/src/transport/account.rs |
| `backend/crates/providers/antigravity/src/transport/client.rs` | modified | E06/E07/E08 · 接入 | backend/crates/providers/antigravity/src/transport/client.rs |
| `backend/crates/providers/antigravity/src/transport/headers.rs` | modified | E06/E07/E08 · 接入 | backend/crates/providers/antigravity/src/transport/headers.rs |
| `backend/crates/providers/antigravity/src/transport/mod.rs` | modified | E06/E07/E08 · 原样 | backend/crates/providers/antigravity/src/transport/mod.rs |
| `backend/crates/providers/antigravity/src/transport/pool.rs` | modified | E06/E07/E08 · 原样 | backend/crates/providers/antigravity/src/transport/pool.rs |
| `backend/crates/providers/antigravity/src/transport/request.rs` | added | E06/E07/E08 · 接入 | backend/crates/providers/antigravity/src/transport/request.rs |
| `backend/crates/providers/antigravity/src/transport/response.rs` | modified | E06/E07/E08 · 接入 | backend/crates/providers/antigravity/src/transport/response.rs |
| `backend/crates/providers/antigravity/tests/credential/quota/service.rs` | modified | E06/E07/E08 · 接入 | backend/crates/providers/antigravity/tests/credential/quota/service.rs |
| `backend/crates/providers/antigravity/tests/profile/selection.rs` | modified | E06/E07/E08 · 原样 | backend/crates/providers/antigravity/tests/profile/selection.rs |
| `backend/crates/providers/antigravity/tests/provider/affinity.rs` | modified | E06/E07/E08 · 接入 | backend/crates/providers/antigravity/tests/provider/affinity.rs |
| `backend/crates/providers/antigravity/tests/provider/execution.rs` | modified | E06/E07/E08 · 接入 | backend/crates/providers/antigravity/tests/provider/execution.rs |
| `backend/crates/providers/antigravity/tests/provider/headers.rs` | modified | E06/E07/E08 · 原样 | backend/crates/providers/antigravity/tests/provider/headers.rs |
| `backend/crates/providers/antigravity/tests/provider/middleware.rs` | modified | X1 · 排除 | 本次增量只验证插件显式改写协议；本地没有该中间件。原生 Chat/GenAI 覆盖见 tests/provider/execution.rs 与 tests/provider/headers.rs |
| `backend/crates/providers/antigravity/tests/transport/client.rs` | modified | E06/E07/E08 · 原样 | backend/crates/providers/antigravity/tests/transport/client.rs |
| `backend/crates/providers/antigravity/tests/transport/count.rs` | modified | E06/E07/E08 · 原样 | backend/crates/providers/antigravity/tests/transport/count.rs |
| `backend/crates/providers/antigravity/tests/transport/mod.rs` | modified | E06/E07/E08 · 原样 | backend/crates/providers/antigravity/tests/transport/mod.rs |
| `backend/crates/providers/antigravity/tests/transport/request.rs` | added | E06/E07/E08 · 原样 | backend/crates/providers/antigravity/tests/transport/request.rs |
| `backend/crates/providers/openai/src/admin/provider.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/admin/mod.rs |
| `backend/crates/providers/openai/src/admin/quota.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/admin/mod.rs |
| `backend/crates/providers/openai/src/compat/chat.rs` | added | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/provider/chat.rs |
| `backend/crates/providers/openai/src/compat/messages.rs` | removed | E02/E05/E12 · 退役 | 已删除；Codex Chat、压缩、请求画像/在线设置 |
| `backend/crates/providers/openai/src/compat/mod.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/compat/mod.rs |
| `backend/crates/providers/openai/src/config.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/config.rs |
| `backend/crates/providers/openai/src/credential/catalog/mod.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/credential/catalog/mod.rs |
| `backend/crates/providers/openai/src/credential/catalog/refresh.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/credential/catalog/refresh.rs |
| `backend/crates/providers/openai/src/credential/selector.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/credential/selector/mod.rs |
| `backend/crates/providers/openai/src/lib.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/lib.rs |
| `backend/crates/providers/openai/src/profile/identity.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/transport/profile/identity.rs |
| `backend/crates/providers/openai/src/profile/state.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/transport/profile/mod.rs |
| `backend/crates/providers/openai/src/provider/chat/headers.rs` | added | X2 · 排除 | 上游 API Key Messages 账号不接入；OAuth 负向契约见 E02 |
| `backend/crates/providers/openai/src/provider/chat/mod.rs` | added | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/provider/chat.rs |
| `backend/crates/providers/openai/src/provider/chat/response.rs` | added | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/provider/chat.rs |
| `backend/crates/providers/openai/src/provider/execution.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/provider/execution.rs |
| `backend/crates/providers/openai/src/provider/failure.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/provider/failure.rs |
| `backend/crates/providers/openai/src/provider/live.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/provider/live.rs |
| `backend/crates/providers/openai/src/provider/messages/count.rs` | modified | X2 · 排除 | 上游 API Key Messages 账号不接入；OAuth 负向契约见 E02 |
| `backend/crates/providers/openai/src/provider/messages/mod.rs` | modified | X2 · 排除 | 上游 API Key Messages 账号不接入；OAuth 负向契约见 E02 |
| `backend/crates/providers/openai/src/provider/messages/translator.rs` | removed | X2 · 排除 | 上游 API Key Messages 账号不接入；OAuth 负向契约见 E02 |
| `backend/crates/providers/openai/src/provider/mod.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/provider/mod.rs |
| `backend/crates/providers/openai/src/provider/observation.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/provider/observation.rs |
| `backend/crates/providers/openai/src/transport/catalog.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/transport/catalog.rs |
| `backend/crates/providers/openai/src/transport/client/backend.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/transport/client.rs |
| `backend/crates/providers/openai/src/transport/client/live.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/transport/client_live.rs |
| `backend/crates/providers/openai/src/transport/client/stream.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/transport/client_sse.rs |
| `backend/crates/providers/openai/src/transport/headers.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/transport/headers.rs |
| `backend/crates/providers/openai/src/transport/protocol/responses.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/transport/protocol/responses.rs |
| `backend/crates/providers/openai/src/transport/request.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/transport/request/mod.rs |
| `backend/crates/providers/openai/src/transport/response_meta.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/transport/response_meta.rs |
| `backend/crates/providers/openai/src/transport/websocket/coordinator.rs` | modified | E10/E13 · 接入 | backend/crates/providers/openai/src/transport/websocket/coordinator.rs |
| `backend/crates/providers/openai/src/transport/websocket/exchange/stream.rs` | modified | E10/E13 · 接入 | backend/crates/providers/openai/src/transport/websocket/exchange/stream.rs |
| `backend/crates/providers/openai/src/transport/websocket/handshake.rs` | modified | E10/E13 · 接入 | backend/crates/providers/openai/src/transport/websocket/handshake.rs |
| `backend/crates/providers/openai/src/transport/websocket/mod.rs` | modified | E10/E13 · 接入 | backend/crates/providers/openai/src/transport/websocket/mod.rs |
| `backend/crates/providers/openai/src/transport/websocket/pool/capacity.rs` | modified | E10/E13 · 接入 | backend/crates/providers/openai/src/transport/websocket/pool/capacity.rs |
| `backend/crates/providers/openai/src/transport/websocket/pool/lease.rs` | modified | E10/E13 · 接入 | backend/crates/providers/openai/src/transport/websocket/pool/lease.rs |
| `backend/crates/providers/openai/src/transport/websocket/pool/mod.rs` | modified | E10/E13 · 接入 | backend/crates/providers/openai/src/transport/websocket/pool/mod.rs |
| `backend/crates/providers/openai/src/transport/websocket/pool/state.rs` | modified | E10/E13 · 接入 | backend/crates/providers/openai/src/transport/websocket/pool/state.rs |
| `backend/crates/providers/openai/src/transport/websocket/pool/supervisor.rs` | modified | E10/E13 · 接入 | backend/crates/providers/openai/src/transport/websocket/pool/supervisor.rs |
| `backend/crates/providers/openai/tests/admin/provider.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/tests/admin/mod.rs |
| `backend/crates/providers/openai/tests/config.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/tests/config.rs |
| `backend/crates/providers/openai/tests/credential/catalog/contract.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/tests/credential/catalog/mod.rs |
| `backend/crates/providers/openai/tests/credential/catalog/refresh.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/tests/credential/catalog/mod.rs |
| `backend/crates/providers/openai/tests/credential/contract/capacity.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/tests/credential/contract/mod.rs |
| `backend/crates/providers/openai/tests/credential/contract/cooldown_reads.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/tests/credential/contract/mod.rs |
| `backend/crates/providers/openai/tests/credential/contract/mod.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/tests/credential/contract/mod.rs |
| `backend/crates/providers/openai/tests/credential/contract/scheduling_projection.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/tests/credential/contract/scheduling_projection.rs |
| `backend/crates/providers/openai/tests/provider/contract/catalog_startup.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/tests/provider/contract/model_account_eligibility.rs |
| `backend/crates/providers/openai/tests/provider/contract/guardian_model.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/tests/provider/contract/guardian_routing.rs |
| `backend/crates/providers/openai/tests/provider/contract/messages/affinity.rs` | modified | E02 · 退役 | 旧 OAuth Messages 转换退役；本地 tests/provider/contract/messages.rs 验证三调度模式拒绝，chat.rs 与 affinity_modes.rs 保留 Chat/原生亲和 |
| `backend/crates/providers/openai/tests/provider/contract/messages/mod.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/tests/provider/contract/messages.rs |
| `backend/crates/providers/openai/tests/provider/contract/messages/translator.rs` | modified | E02 · 退役 | OAuth 原生 Messages 转换退役；目标 Chat 转换由 gateway-protocol/tests/chat_completions 与 provider-openai/tests/provider/contract/chat.rs 覆盖 |
| `backend/crates/providers/openai/tests/provider/contract/mod.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/tests/provider/contract/mod.rs |
| `backend/crates/providers/openai/tests/provider/contract/selection_retry.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/tests/provider/contract/selection_retry.rs |
| `backend/crates/providers/openai/tests/provider/contract/transport.rs` | added | E02/E05/E12 · 接入 | backend/crates/providers/openai/tests/provider/contract/transport.rs |
| `backend/crates/providers/openai/tests/provider/live.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/tests/provider/live.rs |
| `backend/crates/providers/openai/tests/transport/mod.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/tests/transport/mod.rs |
| `backend/crates/providers/openai/tests/transport/profile_contract.rs` | modified | E02/E05/E12 · 接入 | backend/crates/providers/openai/tests/transport/profile_contract.rs |
| `backend/crates/providers/openai/tests/transport/websocket_settings.rs` | added | E02/E05/E12 · 接入 | backend/crates/providers/openai/src/transport/websocket/pool/lifecycle_tests.rs |
| `backend/crates/providers/xai/src/admin/quota.rs` | modified | E11 · 接入 | backend/crates/providers/xai/src/admin.rs |
| `backend/crates/providers/xai/src/lib.rs` | modified | E11 · 接入 | backend/crates/providers/xai/src/lib.rs |
| `backend/crates/providers/xai/src/provider/chat/mod.rs` | added | E11 · 接入 | backend/crates/providers/xai/src/provider/chat/mod.rs |
| `backend/crates/providers/xai/src/provider/chat/response.rs` | added | E11 · 原样 | backend/crates/providers/xai/src/provider/chat/response.rs |
| `backend/crates/providers/xai/src/provider/chat/tools.rs` | added | E11 · 原样 | backend/crates/providers/xai/src/provider/chat/tools.rs |
| `backend/crates/providers/xai/src/provider/continuation.rs` | modified | E11 · 接入 | backend/crates/providers/xai/src/provider/continuation.rs |
| `backend/crates/providers/xai/src/provider/failure.rs` | modified | E11 · 接入 | backend/crates/providers/xai/src/provider/failure.rs |
| `backend/crates/providers/xai/src/provider/mod.rs` | modified | E11 · 接入 | backend/crates/providers/xai/src/provider/mod.rs |
| `backend/crates/providers/xai/src/provider/stream.rs` | modified | E11 · 接入 | backend/crates/providers/xai/src/provider/stream.rs |
| `backend/crates/providers/xai/src/reasoning_replay.rs` | removed | E11 · 退役 | 已删除；原生 wire、Chat 专用转换、计费与旧会话兼容 |
| `backend/crates/providers/xai/src/transport/canonical.rs` | modified | E11 · 接入 | backend/crates/providers/xai/src/transport/canonical.rs |
| `backend/crates/providers/xai/src/transport/compaction.rs` | removed | E11 · 退役 | 已删除；原生 wire、Chat 专用转换、计费与旧会话兼容 |
| `backend/crates/providers/xai/src/transport/mod.rs` | modified | E11 · 接入 | backend/crates/providers/xai/src/transport/mod.rs |
| `backend/crates/providers/xai/src/transport/request/chat.rs` | added | E11 · 原样 | backend/crates/providers/xai/src/transport/request/chat.rs |
| `backend/crates/providers/xai/src/transport/request/history.rs` | removed | E11 · 退役 | 已删除；原生 wire、Chat 专用转换、计费与旧会话兼容 |
| `backend/crates/providers/xai/src/transport/request/identity.rs` | modified | E11 · 原样 | backend/crates/providers/xai/src/transport/request/identity.rs |
| `backend/crates/providers/xai/src/transport/request/mod.rs` | modified | E11 · 原样 | backend/crates/providers/xai/src/transport/request/mod.rs |
| `backend/crates/providers/xai/src/transport/request/parameters.rs` | removed | E11 · 退役 | 已删除；原生 wire、Chat 专用转换、计费与旧会话兼容 |
| `backend/crates/providers/xai/src/transport/request/response.rs` | removed | E11 · 退役 | 已删除；原生 wire、Chat 专用转换、计费与旧会话兼容 |
| `backend/crates/providers/xai/src/transport/request/schema.rs` | removed | E11 · 退役 | 已删除；原生 wire、Chat 专用转换、计费与旧会话兼容 |
| `backend/crates/providers/xai/src/transport/request/tools.rs` | removed | E11 · 退役 | 已删除；原生 wire、Chat 专用转换、计费与旧会话兼容 |
| `backend/crates/providers/xai/tests/provider/contract/chat.rs` | added | E11 · 接入 | backend/crates/providers/xai/tests/provider/contract/chat.rs |
| `backend/crates/providers/xai/tests/provider/contract/mod.rs` | modified | E11 · 接入 | backend/crates/providers/xai/tests/provider/contract/mod.rs |
| `backend/crates/providers/xai/tests/provider/contract/outbound.rs` | modified | X1 · 排除 | 本次增量只移除插件改写 HTTP 方法/字节测试中的 compaction 分支；本地原生 transport 测试与 compact unsupported 回归承担实际入口 |
| `backend/crates/providers/xai/tests/provider/contract/replay_tool_names.rs` | removed | E11 · 退役 | 已删除；原生 wire、Chat 专用转换、计费与旧会话兼容 |
| `backend/crates/providers/xai/tests/transport/canonical.rs` | modified | E11 · 接入 | backend/crates/providers/xai/tests/transport/canonical.rs |
| `backend/crates/providers/xai/tests/transport/compaction.rs` | removed | E11 · 退役 | 已删除；原生 wire、Chat 专用转换、计费与旧会话兼容 |
| `backend/crates/providers/xai/tests/transport/fixtures/grok_compaction_request.json` | removed | E11 · 退役 | 已删除；原生 wire、Chat 专用转换、计费与旧会话兼容 |
| `backend/crates/providers/xai/tests/transport/mod.rs` | modified | E11 · 接入 | backend/crates/providers/xai/tests/transport/mod.rs |
| `backend/crates/providers/xai/tests/transport/request/hosted_names.rs` | removed | E11 · 退役 | 已删除；原生 wire、Chat 专用转换、计费与旧会话兼容 |
| `backend/crates/providers/xai/tests/transport/request/incremental_tools.rs` | removed | E11 · 退役 | 已删除；原生 wire、Chat 专用转换、计费与旧会话兼容 |
| `backend/crates/providers/xai/tests/transport/request/mod.rs` | modified | E11 · 原样 | backend/crates/providers/xai/tests/transport/request/mod.rs |
| `backend/crates/providers/xai/tests/transport/request/parameters.rs` | removed | E11 · 退役 | 已删除；原生 wire、Chat 专用转换、计费与旧会话兼容 |
| `backend/crates/providers/xai/tests/transport/request/web_search.rs` | removed | E11 · 退役 | 已删除；原生 wire、Chat 专用转换、计费与旧会话兼容 |
| `backend/migrations/.frozen-sha256` | modified | E09/E14 · 接入 | backend/migrations/.frozen-sha256 |
| `backend/migrations/0047_request_plan_display.sql` | added | E09/E14 · 接入 | backend/migrations/0103_request_plan_display.sql |
| `backend/migrations/0048_openai_runtime_settings.sql` | added | E09/E14 · 接入 | backend/migrations/0104_openai_runtime_settings.sql |
| `deploy/README.md` | modified | E01/E15 · 接入 | deploy/README.md |
| `deploy/config.example.yaml` | modified | E01/E15 · 接入 | deploy/config.example.yaml |
| `docs/api.md` | modified | E01/E15 · 接入 | docs/api.md |
| `docs/architecture.md` | modified | E01/E15 · 接入 | docs/architecture.md |
| `docs/development.md` | modified | E17 · 接入 | docs/architecture.md 的开发与协议边界；沿用本地 release/toolchains.rb 的工具链权威，不引入插件联调要求 |
| `docs/plugins.md` | modified | X1 · 排除 | 插件专属运行时/中间件排除；公共交付见 E04/E10 |
| `docs/theme.md` | modified | E01/E15 · 接入 | docs/theme.md |
| `frontend/src/api/modules/settings/index.ts` | modified | E15 · 接入 | frontend/src/api/modules/settings.ts |
| `frontend/src/components/usage-records-table/columns.ts` | modified | E15 · 接入 | frontend/src/views/usage/constants.ts |
| `frontend/src/components/usage-records-table/index.vue` | modified | E15 · 接入 | frontend/src/views/usage/components/UsageRecordsTable.vue |
| `frontend/src/i18n/locales/en/accounts.json` | modified | E15 · 接入 | frontend/src/i18n/locales/en/accounts.json |
| `frontend/src/i18n/locales/en/settings.json` | modified | E15 · 接入 | frontend/src/i18n/locales/en/settings.json |
| `frontend/src/i18n/locales/zh-Hans/accounts.json` | modified | E15 · 接入 | frontend/src/i18n/locales/zh-Hans/accounts.json |
| `frontend/src/i18n/locales/zh-Hans/settings.json` | modified | E15 · 接入 | frontend/src/i18n/locales/zh-Hans/settings.json |
| `frontend/src/i18n/locales/zh-Hant/accounts.json` | modified | E15 · 接入 | frontend/src/i18n/locales/zh-Hant/accounts.json |
| `frontend/src/i18n/locales/zh-Hant/settings.json` | modified | E15 · 接入 | frontend/src/i18n/locales/zh-Hant/settings.json |
| `frontend/src/views/accounts/components/account-quota-panel/ResetCredits.vue` | modified | E15 · 接入 | frontend/src/views/accounts/components/AccountQuotaPanel/Credits.vue |
| `frontend/src/views/accounts/components/account-quota-panel/index.vue` | modified | E15 · 接入 | frontend/src/views/accounts/components/AccountQuotaPanel/index.vue |
| `frontend/src/views/settings/components/OpenAiRequestCard.vue` | added | E15 · 接入 | frontend/src/views/settings/components/OpenAiRequestSettingsCard.vue |
| `frontend/src/views/settings/components/RequestCountryCard.vue` | removed | E12/E15 · 接入 | 保留为 OpenAiRequestSettingsCard.vue 子组件；v-model 改接 openaiRequestSettings.requestCountry，不保留第二套设置状态 |
| `frontend/src/views/settings/components/ResetCreditCard.vue` | modified | E15 · 接入 | frontend/src/views/accounts/components/AccountQuotaPanel/AutoReset.vue |
| `frontend/src/views/settings/components/WebSocketPoolCard.vue` | added | E15 · 接入 | frontend/src/views/settings/components/OpenAiRequestSettingsCard.vue |
| `frontend/src/views/settings/components/backup/BackupScheduleCard.vue` | modified | E15 · 接入 | frontend/src/views/settings/components/backup/BackupScheduleCard.vue |
| `frontend/src/views/settings/components/pricing/PricingRateFields.vue` | modified | E15 · 接入 | frontend/src/views/settings/components/pricing/PricingRateFields.vue |
| `frontend/src/views/settings/composables/useSettingsForm.ts` | modified | E15 · 接入 | frontend/src/views/settings/composables/useSettingsForm.ts |
| `frontend/src/views/settings/composables/useSettingsFormRules.ts` | modified | E15 · 接入 | frontend/src/views/settings/composables/useSettingsForm.ts |
| `frontend/src/views/settings/index.vue` | modified | E15 · 接入 | frontend/src/views/settings/index.vue |
| `frontend/src/views/usage/components/UsageDiagnosticCard.vue` | modified | E15 · 接入 | frontend/src/views/usage/components/UsageDiagnosticCard.vue |
| `release/notes.md` | modified | E17 · 接入 | release/notes.md |
| `release/version.yaml` | modified | E17 · 接入 | release/version.yaml |
