# v3.57.1 官方增量接入记录

## 固定范围

本地基线 `c1e9a29ffaa28816bc4d05419066e6c85cdb98f4`，上次官方目标 `db22550e6172e7a3537eb53441ae9ce73bc50cd5`，本次官方目标 `8acf376091ab71774acab54440c25750c119bb69`。固定两端文件树与 compare 路径逐项核对，共 8 个提交、39 个文件项；重命名前后分别计算为 40 条路径。没有依赖或迁移增量，不把尚未部署的本地基线当作生产版本。

| 官方提交 | 内容与本地处置 |
| --- | --- |
| `aad6304a` | WS 两种排除头隔离、握手模型 ETag 首次生成确认，E01/E02 |
| `4bdb15cc` | AGY 基础模型组独立搜索，E03 |
| `eece08a4` | v3.57.1 版本，E07 |
| `a51395bb` | 多平台 README，E07 |
| `51733141` | 官方发行通道；保留 NewCPR 独立发布，E07 |
| `81dacf60` | 不再特殊提取/屏蔽 x-codex-turn-id，E04 |
| `62e9cd03` | 官方设置布局；本地已有独立卡片，E08 |
| `8acf3760` | 压缩异步限并发，E05；插件 middleware 前后解码复用，E06 |

上述变更均位于上次固定官方目标之后：WS/压缩属于官方后续修复，基础组独立搜索和轮次头属于新功能或兼容变化，不能归为上轮遗漏。

## 入口、调用链与错误边界

| 编号 | 接入及验收范围 |
| --- | --- |
| E01 | OpenAI 最终 opening headers→pool/state.rs→Eq/Hash/精确归属/出口诊断。两种排除头名称和全部值参与连接身份；变化后精确续接沿既有 ReplayRequired→previous_response_not_found 要求完整重放，不能复用旧 socket。保留账号、线程、凭据、下游 owner、出口和物理容量。 |
| E02 | client_sse.rs 完成正文与隐私规则投影后，response_create_opening 冻结出站 generate 标记，经 coordinator.rs 传给 exchange/stream.rs。预热仅去掉本次交付副本 ETag；首次生成去掉池内副本 ETag，当前交付保留。响应正文和 turn-state 仍按轮次处理。ETag 回归包含 0/2 次预热及各 40 轮复用。 |
| E03 | API GenAI 解码生成 stream 上下文→protocol::is_standalone_web_search→Core Operation→RuntimeSnapshot::plan_inner→ProviderCandidate→AGY execution。仅基础模型组、非流式、唯一 googleSearch/google_search 对象、没有 thinkingConfig 可免档位；同账号须同时拥有获准具体成员和搜索目标。显式型号保持 agent，选定搜索候选才使用 web_search。无搜索资格、分组拒绝、非法档位分别沿原 RoutingError 分类；不跨账号拼接能力、不绕过输出上限。Chat 强制搜索继续通过真实路由生成候选。 |
| E04 | API OpenAiRequestHeaders 不再生成 context.turn_id，普通 opaque 头保留；OpenAI HTTP/WS headers 和 Live bootstrap/sideband/hangup 不额外丢弃此头。正文、client_metadata、标准 turn metadata 解析及本地身份隔离保持。入口和实际 HTTP/WS/Live 回环均检查。 |
| E05 | Codex HTTP 压缩分支→compress_request_body→Semaphore→spawn_blocking→AbortOnDropHandle。排队取消不发送上游；实际压缩结束前许可仍由工作线程持有；错误仍为 RequestCompression，原发送状态/重试规则不变。Excel 与 Guardian sampler 免压缩分支保留。 |
| E06 | 官方把插件 middleware 前后双解码改为可重用请求。NewCPR 没有该 middleware：Responses HTTP 在鉴权后直接 decode_request_with_headers_limit，GenAI/Chat 各构造一次网关请求，WS 每帧调用一次入口解码。Chat 与固定官方版本都先解析正文校验字段，再由 from_json_bytes 解析并保留原始字节。本次不引入插件或另建缓存；原生入口验证不被插件专属排除吞掉。 |
| E07 | README、API/架构、当前规范、release/version.yaml/notes.md 同步。published 保持为空；v3.57.1 源码接入时的升级计划以 v3.55.3 为起点，仅含 0103/0104；发布前按实际运行基线修订为 v3.54.5 起的 0101–0104，已有迁移字节不变，recovery=manual。源码接入和计划修订不代表已经部署。 |
| E08 | 本地 settings/index.vue 已有 RuntimeSettingsCard、OpenAiRequestSettingsCard 和独立功能卡片，请求解压上限由 RuntimeSettingsCard 的 responsesMaxDecompressedBodyBytes 承担，客户端版本卡仍在同页独立显示。官方导航卡片移动和三语言文案不机械套入不同布局，无前端运行时代码变更。 |

## 保留差异

- 按用户确定的 NewCPR 合同，每账号有效并发仅限制同时 opening；成功、失败、取消释放建连名额，普通请求并发不变。busy/idle/closing 不占 opening，物理出口连接许可仍随 socket 销毁释放，空闲/年龄/断开回收保留。
- 本地 RuntimeSnapshot 保留 restricted_to_accounts、单个 generation_protocol、model_presentation 及账号输出上限；官方来源/搜索目标遍历规则适配到这些合同，没有插件 contribution。
- ETag 修改保留本地 session proxy hold、ResponseControl 取消与终态前回池，未用官方较简化的生命周期覆盖本地行为。
- 插件专属运行时、上游 API Key 账号、主动互联转发按已批准范围排除；OAuth 只读共享、Excel/State、严格排队、Fast/Ultra Fast、Codex 15 套餐和多标签保留。
- 本轮未使用生产流量；实际付费上游、真实 OAuth 刷新及生产部署不在隔离验收范围。以下回归不能等同于整个官方服务逐字节相同。

## 验证记录

测试、编译与格式化仅在指定隔离 Linux 验证主机执行。本地只做编辑、Git 和隐私检查，真实配置和原始日志在仓库外。

- 失败前：固定本地基线仅叠加官方新增回归，5 项均实际执行并失败：Core 搜索权限错误分类、AGY 无档位独立搜索、WS 排除头变更续接、复用 ETag 重复确认、同步压缩无法排队取消。编译通过后取得失败证据。
- 轮次头失败前：API 仍额外生成 context.turn_id，HTTP/WS 实际出站仍丢弃该头，新增 3 项回归均失败；接入后这 3 项及 Live bootstrap/sideband/hangup 同头透传均通过。
- 首轮接入后 ETag 预热用例等待超时，定位为测试专用 websocket_only_request helper 给预热创建了另一 downstream owner；按官方预热方式使用 generate=false 的必选 WS，保持同一 owner，不放宽生产隔离；修正后 0/2 次预热的两组 40 轮均通过。

| 验证层 | 结果 |
| --- | --- |
| API | 2 项单元 + 666 项集成测试通过 |
| Core | 7 项单元 + 586 项集成测试通过 |
| 协议 | 194 项通过 |
| Antigravity | 111 项通过，包含原生搜索、Chat 搜索参数拒绝和 grounding 交付 |
| OpenAI | 347 项单元 + 1369 项集成测试通过，包含每账号 opening、空闲/关闭/物理容量、严格排队、Excel/State、身份与续接 |
| 应用与架构 | 62 项通过；真实隔离 PostgreSQL/Redis 已配置，包含账号重登/重导入、设备身份和架构布局验证 |
| 部署脚本 | 23 项回归通过；升级计划的两个迁移树 ID 与对应源码一致，本轮没有修改 SQL |
| 静态检查与源码 | OVH 全 workspace fmt 和 `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` 通过；2188 个前后端文件 SHA-256 与本地全部一致 |
| 隐私 | 精确暂存内容及 HEAD 全部可达历史扫描通过 |

前端运行时代码无变更，复用上一轮合格的构建和浏览器证据。隔离模拟上游测试不代表真实付费上游验证。

本记录证明上述源码接入及隔离验证，不代替 GitHub 发布、验证镜像或生产部署记录。

## 提交后复查

- E01：本地出口选择曾用隐私改写前的排除头查找已有连接。隔离 IPv6 回环复现了每次新建出口时，即使最终握手排除头相同，连接本地的 `previous_response_id` 仍会丢失。现在先用响应 ID 和原账号、客户端、会话及出口范围定位唯一候选，再按最终握手头领取物理连接。更改最终排除头仍要求重放。此处修复的是 NewCPR 出口选择与官方排除头规则的衔接。
- E02：固定官方版本和首次移植均在正文隐私规则执行后读取原始请求的 `generate`。隔离回环确认，规则把正式请求改成预热时会提前交付握手 ETag，反向改写会错过首次交付。现在使用实际发给上游的 `response.create` 的值。
- OVH 在本批首次提交上分别复现上述两项失败。修复回归使用模拟上游读取实际握手头和帧，验证预热、首次生成、后续复用，以及每次新建出口下排除头相同和不同时的续接结果。
- OpenAI Provider 整套首轮另有一条已有大帧上传用例在等待 Pong 时先读到了末帧，单跑通过。调整模拟上游的接收缓冲，并在读到首字节时一次写入错误和 Ping。保留 Pong 必须先于末帧、上传超时后仍交付原错误的断言。最终 350 项 Provider 单元测试、1369 项集成测试全部通过，workspace fmt 和 Provider 全目标、全特性 Clippy 通过。10 个变更 Rust 文件与隔离验证源码逐项哈希一致。

## 全部官方路径处置

| 官方路径 | 编号与处置 | 本地落点/依据 |
| --- | --- | --- |
| `README.md` | E07 · 接入 | README.md |
| `backend/crates/gateway-api/src/data_plane/codex/request.rs` | E04 · 接入 | backend/crates/gateway-api/src/openai/responses/request.rs |
| `backend/crates/gateway-api/src/data_plane/genai/mod.rs` | E06 · 无需迁移 | backend/crates/gateway-api/src/openai/genai/mod.rs |
| `backend/crates/gateway-api/src/data_plane/generation/http.rs` | E06 · 无需迁移 | backend/crates/gateway-api/src/openai/responses/http.rs；openai/chat/http.rs |
| `backend/crates/gateway-api/src/data_plane/generation/mod.rs` | E06 · 无需迁移 | backend/crates/gateway-api/src/openai/responses/http.rs；openai/chat/http.rs |
| `backend/crates/gateway-api/src/data_plane/responses/request.rs` | E06 · 无需迁移 | backend/crates/gateway-api/src/openai/responses/request.rs |
| `backend/crates/gateway-api/src/data_plane/responses/websocket/mod.rs` | E06 · 无需迁移 | backend/crates/gateway-api/src/openai/responses/websocket/mod.rs |
| `backend/crates/gateway-api/tests/data_plane/chat_completions/http.rs` | E06 · 无需迁移 | 无插件 middleware；由现有 API 原生 HTTP/WS 及解码测试覆盖 |
| `backend/crates/gateway-api/tests/data_plane/middleware/fixture.rs` | E06 · 无需迁移 | 无插件 middleware；由现有 API 原生 HTTP/WS 及解码测试覆盖 |
| `backend/crates/gateway-api/tests/data_plane/responses/websocket/forward.rs` | E06 · 无需迁移 | 无插件 middleware；由现有 API 原生 HTTP/WS 及解码测试覆盖 |
| `backend/crates/gateway-core/src/operation.rs` | E03 · 接入 | backend/crates/gateway-core/src/operation/mod.rs |
| `backend/crates/gateway-core/src/routing/mod.rs` | E03 · 接入 | backend/crates/gateway-core/src/routing/mod.rs |
| `backend/crates/gateway-core/src/routing/snapshot.rs` | E03 · 接入 | backend/crates/gateway-core/src/routing/snapshot.rs |
| `backend/crates/gateway-core/tests/routing/snapshot.rs` | E03 · 接入 | backend/crates/gateway-core/tests/routing/snapshot.rs |
| `backend/crates/gateway-protocol/src/genai/mod.rs` | E03 · 接入 | backend/crates/gateway-protocol/src/genai/mod.rs |
| `backend/crates/gateway-protocol/src/genai/request.rs` | E03 · 接入 | backend/crates/gateway-protocol/src/genai/request.rs |
| `backend/crates/providers/antigravity/src/provider/execution.rs` | E03 · 接入 | backend/crates/providers/antigravity/src/provider/execution.rs |
| `backend/crates/providers/antigravity/tests/provider/execution.rs` | E03 · 接入 | backend/crates/providers/antigravity/tests/provider/execution.rs |
| `backend/crates/providers/openai/src/transport/client/live.rs` | E04 · 接入 | backend/crates/providers/openai/src/transport/client_live.rs |
| `backend/crates/providers/openai/src/transport/client/stream.rs` | E05/E02 · 接入 | backend/crates/providers/openai/src/transport/client_sse.rs |
| `backend/crates/providers/openai/src/transport/headers.rs` | E04 · 接入 | backend/crates/providers/openai/src/transport/headers.rs |
| `backend/crates/providers/openai/src/transport/websocket/coordinator.rs` | E01/E02 · 接入 | backend/crates/providers/openai/src/transport/websocket/coordinator.rs |
| `backend/crates/providers/openai/src/transport/websocket/exchange/stream.rs` | E01/E02 · 接入 | backend/crates/providers/openai/src/transport/websocket/exchange/stream.rs |
| `backend/crates/providers/openai/src/transport/websocket/pool/state.rs` | E01/E02 · 接入 | backend/crates/providers/openai/src/transport/websocket/pool/state.rs |
| `backend/crates/providers/openai/tests/transport/headers.rs` | E04/E05 · 接入 | backend/crates/providers/openai/tests/transport/headers.rs |
| `backend/crates/providers/openai/tests/transport/profile_contract.rs` | E04 · 接入 | backend/crates/providers/openai/tests/transport/profile_contract.rs |
| `backend/crates/providers/openai/tests/transport/usage.rs` | E04 · 接入 | backend/crates/providers/openai/tests/transport/usage.rs |
| `backend/crates/providers/openai/tests/transport/websocket/metadata.rs` | E01/E02 · 接入 | backend/crates/providers/openai/tests/transport/websocket/metadata.rs |
| `backend/crates/providers/openai/tests/transport/websocket_pool.rs` | E01/E02 · 接入 | backend/crates/providers/openai/tests/transport/websocket_pool.rs |
| `docs/api.md` | E07 · 接入 | docs/api.md |
| `docs/architecture.md` | E07 · 接入 | docs/architecture.md |
| `frontend/src/i18n/locales/en/settings.json` | E08 · 无需迁移 | frontend/src/views/settings/index.vue 及既有独立设置卡片；保留本地语言文件 |
| `frontend/src/i18n/locales/zh-Hans/settings.json` | E08 · 无需迁移 | frontend/src/views/settings/index.vue 及既有独立设置卡片；保留本地语言文件 |
| `frontend/src/i18n/locales/zh-Hant/settings.json` | E08 · 无需迁移 | frontend/src/views/settings/index.vue 及既有独立设置卡片；保留本地语言文件 |
| `frontend/src/views/settings/components/ResponseBodyLimitCard.vue`<br>原路径：`frontend/src/views/settings/components/settings-access/ResponseBodyLimitCard.vue` | E08 · 无需迁移 | frontend/src/views/settings/index.vue 及既有独立设置卡片；保留本地语言文件 |
| `frontend/src/views/settings/components/settings-access/index.vue` | E08 · 无需迁移 | frontend/src/views/settings/index.vue 及既有独立设置卡片；保留本地语言文件 |
| `frontend/src/views/settings/index.vue` | E08 · 无需迁移 | frontend/src/views/settings/index.vue 及既有独立设置卡片；保留本地语言文件 |
| `release/notes.md` | E07 · 接入 | release/notes.md |
| `release/version.yaml` | E07 · 接入 | release/version.yaml；版本 3.57.1，published 保持为空 |

## 累积发布计划核对

发布前只读核对的应用运行版本为 v3.54.5，源码提交 `4455001ad21ad2b4044148a061d16a3f4ece1170`，尚未应用 0101/0102。v3.57.1 计划改为从该版本直接升级，包含 0101–0104，准确绑定迁移树 `2fd74c411348df0a064cec355b90d5ab03fb9936` → `0719d33a1d5063807fc418898e620852e53115d8`。旧 SQL 不变，仅新增四条 SQL 及冻结摘要，失败采用人工恢复。该计划修订不改变上述功能和测试结论，实际部署结果另行核对。

累计计划已按实际旧镜像源码与目标源码核对准确新增路径及迁移树，旧 SQL 未改写。指定隔离 Linux 验证主机的部署工具 23 项回归通过；本次只修改计划和说明，复用已完成的业务代码验证。

发布 CI 发现提交后复查新增的 `projected_ws` 子模块与 `round_robin_tests.rs` 并存，违反既有目录布局规则。指定隔离 Linux 验证主机复现架构检查失败后，将父文件原样移动为 `round_robin_tests/mod.rs`，不改运行代码、断言或架构门禁。14 项架构检查、15 项同模块回归与 workspace 格式检查全部通过。此前 Provider 回归通过未覆盖这一新增目录结构，不能替代本次架构复验。
