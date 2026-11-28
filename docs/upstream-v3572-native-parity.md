# v3.57.2 官方增量接入记录

## 固定范围

本地基线和已合并提交 `840a45ae43aadcd1a19ab868de0bbf3173e64036`，对应部署源码 `47f92bdf73c6c63cf5a74cd7a46b83b5d6608eeb`。前次官方 `8acf376091ab71774acab54440c25750c119bb69`，本次目标 `909b514074a90ebaa73a73c71dbbecbbe6fe91d3`。增量 3 个提交、19 个文件，ahead=3/behind=0；没有迁移、依赖或前端增量。

- `7b04e08d`：AGY 完整目录元数据和生图发现后续修复。
- `533a415a`：版本 3.57.2。
- `909b5140`：官方发行通道；NewCPR 保留自己真实的发布状态。

解压、异步压缩、设置布局及 x-codex-turn-id 已属于前次基线，不重复计入。

## 运行接入与保留差异

| 编号 | 入口、owner、错误边界 |
| --- | --- |
| E01 | CloudCodeClient::catalog_for_project → Catalog/Model → CatalogService::publish。外层与模型扩展保留；持久化去 quotaInfo，沿用本地身份、套餐、registration、代理及 observation 校验。 |
| E02 | discovery_order → account_metadata → RuntimeSnapshot::model_metadata_for_operation → API agent_menu。聊天顺序之后追加生图并去重；隐藏辅助型号只影响菜单。 |
| E03 | API models::catalog → OpenAiService::genai_model_catalog → ExecutionService::client_model_catalog → AGY Provider → CatalogService。完整单来源、冻结账号模型权限、Core 可路由性和别名；API 再筛原生生成/计数能力，Native 要求 GenAI 对象。列表与详情的目录读取失败或原生对象无效时按官方返回 `503 UNAVAILABLE`，成功读取后缺少详情返回 `404 NOT_FOUND`。 |
| E04 | model_groups、quota::document/service、provider::candidates 读取新 metadata/accessor。非法 quotaInfo 返回 Unavailable，不提交部分配额/目录；分组推理、独立搜索与输出上限仍按真实候选账号。 |
| E05 | 旧快照可解析，旧扩展缺失不能造数；旧 agentModelOrder 不当作新声明。既有刷新机制取得完整上游声明；不增加第二套缓存或破坏身份。 |
| E06 | 文档、规范、版本同步；published={}。源码提交与正式发行、部署分开记录。 |

本地 ProviderModelDescriptor 使用 payload，官方 Native content 在此直接接入；无需引入插件的 Adapted content/contributed_models/peer_route/middleware。现有 Core 完整对象、别名与快照检查已满足该原生路径。保留本地共享画像字段与安全共同范围、目录刷新轮转及凭据/代理竞态保护。账号请求并发、WS opening、回收、出口、Excel/State、Fast/Ultra Fast 不在修改范围。

兼容决定已于 2026-11-28 经用户确认：采用官方新版，普通 `/v1beta/models` 列表和详情不再额外补充本地合成基础分组名称，只提供上游实际型号和可映射到它的别名。映射到合成基础组的别名同样不补入目录；依赖模型发现的客户端应选择列表返回的具体型号。基础组指定合法档位的生成调用仍可执行，AGY 专用选择器原来已按具体型号展示，普通 `/v1/models` 不受此项影响。这不是账号分组变更；本轮实施与验证已完成，提交和发布状态单独记录。

## 验证状态

源码提交 `be3648e72f` 在隔离 Linux 主机完成 828 项测试及架构检查，包括 API 单元 2 项、API 集成 668 项、Antigravity 115 项、Core 目录 29 项及架构检查 14 项。API/Antigravity 全目标全特性 Clippy、Core 测试目标 Clippy、workspace `cargo fmt --all --check` 及候选树隐私扫描通过。该提交的 2484 个文件与隔离主机按哈希核对一致。

后续复查对齐了 GenAI 目录错误响应。新增回归在 `be3648e72f` 的实现上得到 500，修复后同一隔离主机的 7 项 GenAI 模型目录测试通过，包含列表和详情的 `503 UNAVAILABLE` 错误正文以及模型不存在时的 `404`。

| 行为 | 回归证据 |
| --- | --- |
| E01/E03 | `native_catalog_preserves_upstream_objects_without_chat_menu_filtering`、`native_catalog_selects_whole_objects_only_from_authorized_sources`；旧版均失败，候选通过 |
| E02/E04 | `model_group_menu_routes_chat_and_native_generation_to_declared_members` 增加官方图像菜单、顺序及隐藏断言，保留本地分组画像与生成正反例 |
| E03 | `native_catalog_preserves_metadata_and_aliases_for_every_client` 在旧 API 运行代码上失败（displayName 被简化成别名），候选通过。`native_catalog_returns_unavailable_for_read_and_payload_errors` 核对列表和详情在 Provider 读取失败、协议错误、对象无效时的 HTTP 状态与完整 Google RPC 正文。`native_catalog_filters_unexecutable_models` 覆盖权限过滤及详情 404。 |
| E03 Core | 复用真实 `DefaultExecutionService` 的 `client_catalog_forwards_scope_maps_whole_objects_and_omits_unroutable_models`，补充 GenAI 协议与错误正反例；保留 Codex 原回归 |
| E04 | `malformed_model_quota_does_not_publish_partial_quota_or_catalog` 核对旧额度与目录正文、账号/revision/套餐/etag/观察时间均未被部分更新 |
| E05 | `legacy_catalog_restores_known_fields_and_refreshes_full_discovery_metadata` 验证旧字段可读、不触发读时联网、后台刷新替换声明及图像去重 |

验证使用模拟上游及现有内存 Store，未改 PostgreSQL/Redis 层，也未执行真实 Google 授权或真实生图调用。不能将此结果描述为真实上游验收或整个平台字节等价。

## 文件账

| 官方文件 | 本地落点 | 处置 |
| --- | --- | --- |
| `backend/crates/gateway-api/src/data_plane/antigravity.rs` | `backend/crates/gateway-api/src/openai/genai/agent_menu.rs` | 注释同步到现有 agent_menu |
| `backend/crates/gateway-api/src/data_plane/genai/models.rs` | `backend/crates/gateway-api/src/openai/genai/models.rs` | 移植并适配本地接口 |
| `backend/crates/gateway-api/tests/data_plane/genai/mod.rs` | `backend/crates/gateway-api/tests/openai/genai/mod.rs` | 移植并适配本地接口 |
| `backend/crates/gateway-api/tests/data_plane/genai/models.rs` | `backend/crates/gateway-api/tests/openai/genai/models.rs` | 移植并适配本地接口 |
| `backend/crates/gateway-core/src/routing/catalog.rs` | `backend/crates/gateway-core/src/routing/catalog.rs` | 移植并适配本地接口 |
| `backend/crates/providers/antigravity/src/credential/catalog.rs` | `backend/crates/providers/antigravity/src/credential/catalog.rs` | 移植并适配本地接口 |
| `backend/crates/providers/antigravity/src/credential/model_groups.rs` | `backend/crates/providers/antigravity/src/credential/model_groups.rs` | 移植并适配本地接口 |
| `backend/crates/providers/antigravity/src/credential/quota/document.rs` | `backend/crates/providers/antigravity/src/credential/quota/document.rs` | 移植并适配本地接口 |
| `backend/crates/providers/antigravity/src/credential/quota/service.rs` | `backend/crates/providers/antigravity/src/credential/quota/service.rs` | 移植并适配本地接口 |
| `backend/crates/providers/antigravity/src/provider/candidates.rs` | `backend/crates/providers/antigravity/src/provider/candidates.rs` | 移植并适配本地接口 |
| `backend/crates/providers/antigravity/src/provider/mod.rs` | `backend/crates/providers/antigravity/src/provider/mod.rs` | 移植并适配本地接口 |
| `backend/crates/providers/antigravity/src/transport/client.rs` | `backend/crates/providers/antigravity/src/transport/client.rs` | 移植并适配本地接口 |
| `backend/crates/providers/antigravity/tests/credential/catalog.rs` | `backend/crates/providers/antigravity/tests/credential/catalog.rs` | 移植并适配本地接口 |
| `backend/crates/providers/antigravity/tests/provider/execution.rs` | `backend/crates/providers/antigravity/tests/provider/execution.rs` | 移植并适配本地接口 |
| `backend/crates/providers/antigravity/tests/transport/client.rs` | `backend/crates/providers/antigravity/tests/transport/client.rs` | 移植并适配本地接口 |
| `docs/api.md` | `docs/api.md` | 移植并适配本地接口 |
| `docs/architecture.md` | `docs/architecture.md` | 移植并适配本地接口 |
| `release/notes.md` | `release/notes.md` | 移植并适配本地接口 |
| `release/version.yaml` | `release/version.yaml` | 版本同步，官方 published 通道排除 |

## 本地附加接入路径

- `gateway-api/src/openai/service.rs`：既有 HTTP service adapter 增加 GenAI 目录调用，无插件上下文或第二份鉴权。
- `gateway-core/tests/engine/execution.rs`：在同一完整对象/映射/失败用例中覆盖 GenAI 和 Codex，确认共享 Core 正确转发协议。
- `providers/antigravity/tests/credential/quota/service.rs`：非法 quotaInfo 的整次拒绝与旧成功状态保持。
- `README.md`、`.trellis/spec/backend/native-model-catalog.md`：更新当前接入索引和协议合同。

## 官方行为边界

官方 3.57.2 与本版均存在目录顶层 `project` 字段冲突。上游响应带有该字段时，它与本地保存的项目字段重名，导致目录无法读回。此条件已用合成数据验证。后续合并时以官方修复为准。

`discovery_order` 只在 `agentModelSorts` 是数组时建立声明顺序。缺少声明（包括旧持久快照）时按 unspecified 展示已有具体型号，不凭空补出图像 ID；新目录刷新后才应用图像追加。原生目录的 displayName 和标签取完整来源对象，别名只改资源 name，不用另一账号的菜单字段覆盖它。生图执行能力、客户端分类和实际 Google 账号权限仍取决于真实上游及客户端，不能由发现目录修复保证。
