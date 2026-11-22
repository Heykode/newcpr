# CPR v3.54.3 原生更新接入记录

## 固定范围

- 本地起点：`edf23e85f8aafb62c94fc42211e5b4e4ebe31779`（NewCPR PR #301 合并结果）。
- 上次官方基线：`a1954c47460b5b9017dcb4e409bbd78b22bd9e66`。
- 本次官方目标：`6900da36eedb3099d43c8c9bf5b8b97d82d15688`，固定 compare 为 ahead，9 个提交、15 个文件。
- 本轮接入固定范围的适用增量。发布阶段已获提交、推送、合并、版本标签和免备份切换部署授权；
  版本为 `3.54.3`，`published` 保持为空。部署状态以精确提交的验证镜像与实际部署记录为准。
- 无 SQL 迁移、依赖版本、数据库/Redis 镜像、套餐选项、排队换号、严格亲和、Ultrafast、Guardian 调度改动。

共用政策见 [官方更新合并核对清单](upstream-integration-checklist.md)。

## 行为与调用链

| 项目 | 官方来源 | 本地接入与影响 |
| --- | --- | --- |
| E01 额度周期证据 | `aa34d5291d43ec5f8d58621a5d5e61393acc4b1d`（#760） | `AccountsService::quota_forecast` 读取缓存额度与有界历史，传入每次请求的开始、完成、到期时间。采样按独立请求确认周期，reset 偏移不截掉累计；缺套餐跳过，明确套餐变更才重建 |
| E02 美元后处理 | E01 对本地既有美元公式的必要接入 | `QuotaForecastSample.awaiting_cycle_confirmation` 显式传给 `apply_current_quota_estimates`，按所选窗口暂停美元估算，保留来源累计和不可用提示；确认后恢复原动态公式 |
| E03 账号额度卡片 | `b85f944c52f01f381848c86df982248f3661b070` | `AccountQuotaPanel/index.vue` 在排序、分组前筛选 `accountWide`，桌面弹出卡与手机内嵌卡共用。原始窗口、模型列表和公共排序函数不变 |
| E04 WebSocket 测试时钟 | `d1be306f0322120c68745a2795e9bd918cff0f02`（#761） | `idle_connection_reaches_the_official_limit_without_starting_an_execution` 仅在快进 60 分钟时暂停时钟；真实握手和收帧用实时时钟。保留完整 400 错误、零执行及连接清理断言；生产连接期限不变 |

### E01 / E02 完整路径与保留差异

1. `gateway-api/src/admin/accounts/handlers.rs` 的 `GET /api/admin/accounts/quota-forecast`
   保留 `AdminAuth`、`AccountIdQuery`、`map_service_error` 与 `AdminEnvelope`。
2. `gateway-admin/src/use_case/accounts.rs` 经现有账号身份及 Provider 选择读取 `refresh=false`
   的缓存快照，只对账号级、至多 32 天窗口采样；保留当前窗口、未来观测、百分比有效性和账号创建边界。
3. `gateway-store/src/postgres/provider_accounts/admin_adapter.rs` 调用现有
   `observability/analytics/forecast.rs`，有界历史已提供开始/完成时间和完整区间累计；无需表结构或查询改动。
   原生、Excel 和托管 State 的已完成用量仍归现有统计合同，未创建平行账本。
4. `providers/openai/src/admin/mod.rs::quota_forecast_observation` 复用历史限流解析，核对账号级归属、
   桶、角色和时长。服务层跳过不在自身窗口、请求开始早于查询边界、完成不早于快照的历史事实。
   历史窗口时长在账号创建时间裁剪前计算，使用 checked 时间运算。
5. `quota_forecast_sampling.rs` 的 `cycle_boundary`、`follows` 和 `select_forecast_sample`
   接入官方完整分支。两条独立旧请求的期限均已过去且新比例回落超过 1pp，先暂停；第二条独立新请求
   在首条新读数的窗口内完成且仍回落，才确认换段。快照、复制读数和重叠请求不充当确认。
   过期读数不作剩余预测锚点；同窗口回落仍保留累计。
6. `quota_forecast.rs::account_quota_forecasts` / `project` 保留来源累计及未知值。
   新增内部等待字段供本地美元后处理使用，不新增 wire 字段，也不匹配提示文案。
7. `apply_current_quota_estimates` 只在明确等待时跳过当前窗口美元重算，不以全部 `discontinuous`
   或普通样本不足代替等待条件。正常估值保留“当前窗口 USD 消费 × 100 / 已用百分比”，
   剩余为总量减当前费用。确认后 Token 来源可能为新配对段，USD 来源仍是当前窗口费用；
   这是已批准保留的本地合同，与官方增量美元估值并不等价。缺当前费用不能借历史 USD 补齐。
8. `gateway-api/src/admin/accounts/presenter.rs` 保持来源、null/零、不可用提示投影，前端详情继续使用
   10 秒缓存和打开期间 30 秒轮询。列表动态估值和分组学习不加载配对历史，不取得此详情等待状态。

错误边界保留 Provider 额度读取错误、Store 配对用量错误及当前窗口费用读取错误的原映射；
无额外兜底、刷新请求、调度状态变更或错误吞并。采样模型保留本地字段名 `cached_tokens`，
对应官方 `cached_input_tokens`，不改变已有 Store/API 契约。

### E03 消费者边界

过滤仅位于账号额度卡片。Codex 点数、透支、刷新、个人信息、重置/设置按钮和 Excel 说明保留。
只有模型专属窗口或空窗口时继续显示“额度待观测”，不删除底层额度数据，不增加上游 API Key/互联能力。

## 官方文件逐项映射

| 官方文件 | 本地处理 |
| --- | --- |
| `backend/crates/gateway-admin/src/model/quota_forecast_sampling.rs` | E01，保留本地费用/缓存字段；E02 新增内部等待字段 |
| `backend/crates/gateway-admin/src/use_case/accounts/mod.rs` | E01/E02，接入本地 `src/use_case/accounts.rs` |
| `backend/crates/gateway-admin/tests/model/quota_forecast_sampling.rs` | 官方过期、复制、并发、矛盾读数正反例及新时间字段 |
| `backend/crates/gateway-admin/tests/use_case/accounts/mod.rs` | 本地 `tests/use_case/accounts.rs`：同窗口回落、reset 偏移、确认换段、缺/变套餐、新窗口起点；美元断言遵循本地动态合同 |
| `backend/crates/gateway-api/tests/openai/responses/websocket/connection/mod.rs` | E04 原逻辑接入，保留本地 drop/abort 清理 |
| `deploy/README.md` | 排除互联说明，本轮无部署文档行为变化 |
| `docs/api.md` | 接入预测说明；排除互联按钮说明 |
| `docs/architecture.md` | 接入周期证据，明确本地美元公式及等待范围 |
| `frontend/build/console-banner.ts` | 用户排除控制台 banner，不新增 |
| `frontend/src/views/accounts/components/AccountTableActions.vue` | 排除互联按钮变动 |
| `frontend/src/views/accounts/components/account-quota-panel/index.vue` | E03 接入本地 `AccountQuotaPanel/index.vue`；同文件互联分支排除 |
| `frontend/src/views/accounts/index.vue` | 排除互联回调变动 |
| `frontend/vite.config.ts` | banner 接入排除 |
| `release/notes.md` | 发布阶段补充 v3.54.3 更新内容、保留差异与升级边界 |
| `release/version.yaml` | 发布阶段更新为 3.54.3，`published` 保持为空 |

控制台 banner 来源 `d05d38718e7c`；互联布局来源 `58cbb607505d`。
`8b69fa48820e`、`56d3078ee8b7`、`e9a5ee7e3b00`、`6900da36eedb` 为版本/发行通道更新。
v3.54.2 的国家策略修复在上一轮已接入，不计成本轮新增。

## 验证记录

本机隔离合成数据验证；未使用真实上游凭据或生产流量。原始日志、截图和临时脚本位于仓库外。

| 验证 | 结果 |
| --- | --- |
| 旧源码回归 | reset 偏移用例累计实际 8,100、预期 70,600；账号卡混入模型窗口，均先失败 |
| 仅官方采样、尚无本地美元保护 | 周期来源已保留，但美元仍返回 50、预期 null；证明必须适配实际后处理 |
| 账号服务/模型 | 完整 Admin 主套件 471 项通过；随后补入官方同窗口回落及来源金额保护断言，最终定向 20 项通过（测试总数 472） |
| HTTP 与 Provider | HTTP 鉴权/参数、响应投影 2 项，OpenAI Provider 历史解析与窗口身份 1 项通过 |
| WebSocket 连接 | 27 项通过；空闲期限用例连续 100 次通过 |
| 格式与 Clippy | rustfmt；Admin/API 全 targets、全 features、locked Clippy（`-D warnings`）通过 |
| 架构 | 标准 Cargo 构建后执行架构 14 项、顶层源码纪律 20 项，合计 34 项通过；应用及其依赖完成测试构建 |
| 前端完整测试 | 1,186 项通过 |
| 前端类型、Lint、生产构建 | 通过 |
| 实际浏览器 | Chromium、WebKit，1440/390/320px，混合/仅模型/空窗口共 18 个场景；点数与刷新保留，无页面异常、横向外溢，截图人工复核 |
| 隐私与差异 | 独立候选 index 精确内容隐私检查、`git diff --check` 通过；发布阶段另检查真实 staged 内容和完整发布历史 |

未执行真实 PostgreSQL 数据回放、真实上游或生产运行验收；Store 查询、身份、费用归属和调度没有改动。
本次确认的是固定适用更新及本地接入，不能声明整个服务逐字节等同官方或零缺陷。
发布阶段另核验精确提交 CI、Security Scan、完整发布历史隐私和部署镜像。
