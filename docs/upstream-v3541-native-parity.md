# 官方 v3.54.1 原生增量接入记录

## 固定基线

- 官方旧基线：`da866de2c71c34d13a0a370d070eb923f44a2029`（v3.54.0）。
- 官方目标：`a1954c47460b5b9017dcb4e409bbd78b22bd9e66`（v3.54.1 加发布后的国家策略修复），本次不继续跟随浮动 main。
- NewCPR 本地与调查时主线：`714f5424f85a3a042581ca3dd857aa16dbf47462`；工作分支 `codex/upstream-v3541`。
- 调查时运行版本 v3.54.0，镜像源码 `822fcf49b458b0ff5cd1a8c1795d48e62a73b988`，部署记录主线为上述 merge commit。
- 2026-11-21 核对官方 compare：8 个提交、50 个文件条目，无历史分叉；不表示 50 个文件原样覆盖本地。
- 官方 UI 指针由 `498a491ea97dbb8c5cedbffd97064edab6969341` 到 `501495d5b1e9d65840d4cdee5e0e228f361613ee`。
- 合并政策为 `docs/upstream-integration-checklist.md`。本账记录提交前的本地实施与验证；后续提交、推送、合并和 CI 状态以关联 PR 为准，标签与部署另行记录。

## 官方提交

| 提交 | 更新 | 处理 |
| --- | --- | --- |
| `4db65d8e12cd` | PostgreSQL/Redis 固定摘要 | E09，同步 CI、Compose、备份客户端 |
| `507970019089` | UI v0.13.1 WebKit 表头 | E04，本地没有同一伪元素实现，验证本地组件 |
| `e645d6d4c966` | 时间列统一 | E05，概览及分析共用两行时间 |
| `8041ddab65ff` | 图表、表单、图标、代理整理 | E06/E07/E08，按实际消费者适配 |
| `5a4f49036b8c` | 共用模型权限与独立国家卡片 | E02/E03，复用既有模型编辑器并补帮助 |
| `e9627168a7f0` | 版本 3.54.1 | E10，更新本地版本 |
| `636a3d2a26cf` | 官方 Release 通道 | E10，不导入官方资产，published 保持空 |
| `a1954c47460b` | 国家策略限定 Codex OAuth | E01，显式 OAuth 调用边界 |

## 行为、调用链与差异

| 项目 | 实际入口、helper、调用者与错误边界 | 结论 |
| --- | --- | --- |
| E01 | settings → frozen RequestAttemptContext → Provider 选号 → OAuth 分支中的 `apply_request_country` → HTTP/WS；通用 `align_structured_location_fields` 独立 | 四策略语义不变；认证枚举仅 OAuth，因此没有本地 API Key country 缺陷可复现。Excel 保持独立映射；嵌套业务同名字段不改 |
| E02 | settings/index → OpenAiRequestSettingsCard → RequestCountryCard → BaseForm/useSettingsForm → settings/update；HelpPopover 支持 slot 与原 content prop | 独立卡片已存在，本次抽出组件并加四策略说明、OAuth 范围、三语言及校验文案。保留本地间距，未盲套官方负 margin |
| E03 | AccountSettingsFields、AccountGroupFormModal → AccountModelAccessField；后端 routing snapshot 组合账号/分组权限 | 原本已共用、支持创建时白/黑名单、手填及图片快捷项。本次统一黑名单文案，保留现有路径与 15 种 Codex 账号套餐筛选 |
| E04 | 本地 BaseTable/index 的 sticky th 背景 → 账号/用量等实际表格 | 官方修复删除 thead 的绝对定位 before 背景；本地没有该实现。Chromium/WebKit 固定表头、左右列、展开行及横纵滚动通过，不引入 UI 子模块 |
| E05 | shared usageRecordColumns → useUsageRecordColumns / dashboard UsageRecordCard → UsageRecordsTable → UsageTimeCell | 去掉 requestDetails 页面差异，共享时间宽 128px，保留列 key、偏好、排序与操作列；本地没有 key-usage 页面 |
| E06 | UsageCostCard 接收本地聚合 cost/activity → chart.series → legend/line/tooltip | 共用金额与比例序列，保留金额字符串精度显示、无请求补零、有请求未知金额为 null、缺对照估算不显示 tooltip 行；不复制官方分指标接口 |
| E07 | useProxyBatchCreate/useProxyBatchTest → ProxyBatchResults；proxies/index 批量测试 → 同组件；useProxyBatchTesting → summarizeProxyTests → toast | 共用完成/成功/失败/需处理/跳过计数；连接成功且地区未知单独统计。列表每代理一行，测试错误优先，其次地区失败/冲突，再次创建错误；保留停止、revision、分页与异步失效规则 |
| E08 | ProviderIconGroup 的 sizeClasses → 现有所有消费者；HelpPopover → 新国家帮助及原文本帮助 | 三尺寸查表，保留未知平台/认证标签；无 schema-form 及插件消费者，不新增该功能 |
| E09 | _quality.yml、compose.yaml、Dockerfile POSTGRES_IMAGE | 官方 PG/Redis 摘要同步，修改前仓库外备份。生产 PG/Redis 未升级；没有新迁移、旧迁移字节未变 |
| E10 | release/version.yaml、notes、api/architecture/deploy 文档与本账 | 版本为 3.54.1，published 为空；不是发布或部署完成证据 |

`ProxyBatchResults` 额外接受仅 tests 的实际消费者，避免为批量测试伪造“已添加”行；地区冲突
使用本地 `locationDetectionMessage` 的现有语义。`lastTest.location` 保持安全可选访问，
不采用官方 patch 中对 location 的非可选读取。两项均由本地接口与实际调用者要求。

## 排除范围

- 上游 API Key country 透传分支、API Key/CPR 互联额度入口与发现菜单：本地不提供这些账号路径。
- 插件 schema-form 重构：无实际消费者；不新增插件运行时。
- 官方 UI 依赖、锁文件和子模块：本地 Base 组件承担界面，不替换整套组件库。
- 生产 PostgreSQL/Redis 容器升级：单独计划，不随应用更新重建受保护服务。
- 本轮官方增量没有 SQL 迁移、排队失败换号、严格亲和、Ultrafast 或 Guardian 调度变更。

## 验证记录

本节证据采集于提交前，对应上述基线加本次变更。测试使用本机隔离合成数据，
没有调用真实上游或写入生产。已有同源回归复用，不把 helper 验证等同于实际接口。

| 范围 | 命令或场景 | 结果 |
| --- | --- | --- |
| 前端完整回归 | `node --test tests/*.test.mjs` | 1184 项通过；后续追加通知回归及文案后，定向 proxy/UI/i18n 13 项通过，总测试数增至 1185 |
| 前端静态与构建 | `vue-tsc -b --pretty false`、`eslint .`、`vite build` | 通过；使用已安装的项目二进制，系统 pnpm 9 不支持项目 pnpm 11 workspace 配置 |
| 费用与代理 | upstream-v3541-ui.test.mjs、proxy-batch.test.mjs | 空桶/未知费用/对照提示、三视图/加载、九种批量状态、缺地区对象、错误优先级、通知汇总、停止与结果回填通过 |
| 国家实际出站 | `cargo test -p provider-openai --test main reasoning_replay --locked` | 10 项通过，含 HTTP/WS 四策略、缺失/null、嵌套字段保留与回放回归 |
| 位置优先级 | 同包 `proxy_location_precedence` | 1 项通过，真实本地 HTTP/WS、代理与全局位置及身份保留 |
| 模型权限 | `cargo test -p gateway-core --test main group_model_access_requires_a_common_group_for_account_and_mapped_model --locked` | 1 项通过，账号/分组权限交集 |
| 后端静态 | `cargo fmt --all`、provider-openai 全 targets/features Clippy `-D warnings`、`cargo check --workspace --all-features --locked` | 通过 |
| 架构 | 隔离执行现有 main/architecture 源码检查 | 34 项通过；标准 Cargo 测试构建因 target 缓存文件消失中断，改用相同锁定 syn/serde_json 编译原检查，未执行 bootstrap 业务测试 |
| 配置与部署合同 | ci_workflows_test.rb、toolchains_test.rb、test_ci_plan.py、test_deploy.py | 8+3+15+23 项通过；未执行 Linux Docker、真实新 PG/Redis 镜像和 GitHub CI |
| 迁移冻结 | `shasum -a 256 -c .frozen-sha256`、旧迁移 diff | 全部通过，无迁移变更 |
| 浏览器 | Chromium 与 WebKit 26.5，1440/390/320px 合成消费者 | 时间两行、代理单行结果/地区失败、费用三视图/加载、表头不遮挡、固定列与展开行通过；无页面异常和横向外溢 |
| 设置与分组浏览器 | 两引擎 1440/390px 实际页面、隔离 API | 四策略保存并重新读取、自定义代码拒绝/保存、帮助四说明、创建分组手填黑名单及图片快捷项通过 |
| 修复前后 | 基线组件与当前组件使用相同合成浏览器流程，两引擎 390px | 基线概览无 time 两行、代理地区失败消息不显示；当前均通过。国家策略是边界显式化，未声称本地 API Key 缺陷复现 |
| 隐私 | 独立候选 index 的 `tools/privacy/check.py staged` | 通过，实际 index 不受影响；发布历史扫描留待提交/推送前执行 |

原始日志、截图、临时脚本和配置备份保存在仓库外。发布时仍须重新核验精确提交的 CI、
Security Scan、完整发布历史隐私及已验证镜像。未验证真实上游、生产运行、新基础服务镜像，
不能据本地检查声称全部服务逐字节等价或零故障。

## 官方文件条目归档

下列 50 项以固定 compare 为准，最终行为及验证见上表；“复用”表示已核对本地真实调用者，
不是复制同名文件。

| 官方路径 | 本地落点或排除原因 |
| --- | --- |
| `.github/workflows/_quality.yml` | E09：模板、CI、构建来源同步；不重建生产数据库或 Redis |
| `backend/crates/providers/openai/src/provider/mod.rs` | E01：只接原生 OAuth 适用规则和测试；排除上游 API Key 分支 |
| `backend/crates/providers/openai/src/transport/request.rs` | E01：只接原生 OAuth 适用规则和测试；排除上游 API Key 分支 |
| `backend/crates/providers/openai/tests/provider/contract/mod.rs` | E01：只接原生 OAuth 适用规则和测试；排除上游 API Key 分支 |
| `backend/crates/providers/openai/tests/provider/contract/passthrough.rs` | E01：只接原生 OAuth 适用规则和测试；排除上游 API Key 分支 |
| `deploy/Dockerfile` | E09：模板、CI、构建来源同步；不重建生产数据库或 Redis |
| `deploy/README.md` | E01/E02/E03/E09/E10：同步适用文档，保留 NewCPR 路径和边界 |
| `deploy/compose.yaml` | E09：模板、CI、构建来源同步；不重建生产数据库或 Redis |
| `docs/api.md` | E01/E02/E03/E09/E10：同步适用文档，保留 NewCPR 路径和边界 |
| `docs/architecture.md` | E01/E02/E03/E09/E10：同步适用文档，保留 NewCPR 路径和边界 |
| `frontend/package.json` | E04：已核对 UI 原始 diff 并验证本地 BaseTable；不导入未使用依赖 |
| `frontend/pnpm-lock.yaml` | E04：已核对 UI 原始 diff 并验证本地 BaseTable；不导入未使用依赖 |
| `frontend/src/components/HelpPopover.vue` | E02/E08：本地帮助组件适配 |
| `frontend/src/components/ProviderIconGroup.vue` | E08：本地图标尺寸规则 |
| `frontend/src/components/account/model-access/AccountModelCleanupPopover.vue` | E03：复用既有模型编辑器，统一文案并验证创建、手填及图片快捷项 |
| `frontend/src/components/account/model-access/AccountModelOption.vue` | E03：复用既有模型编辑器，统一文案并验证创建、手填及图片快捷项 |
| `frontend/src/components/account/model-access/index.vue` | E03：复用既有模型编辑器，统一文案并验证创建、手填及图片快捷项 |
| `frontend/src/components/account/model-access/modelAccess.ts` | E03：复用既有模型编辑器，统一文案并验证创建、手填及图片快捷项 |
| `frontend/src/components/schema-form/index.vue` | 排除：本地无组件或消费者，插件功能不引入 |
| `frontend/src/components/usage/UsageRecordsTable.vue` | E05：实际共享用量表格；不存在的独立页面不新增 |
| `frontend/src/components/usage/UsageTimeCell.vue` | E05：实际共享用量表格；不存在的独立页面不新增 |
| `frontend/src/components/usage/shared/columns.ts` | E05：实际共享用量表格；不存在的独立页面不新增 |
| `frontend/src/i18n/locales/en/accounts.json` | E02/E03：适用翻译；排除互联专属文案 |
| `frontend/src/i18n/locales/en/groups.json` | E02/E03：适用翻译；排除互联专属文案 |
| `frontend/src/i18n/locales/en/settings.json` | E02/E03：适用翻译；排除互联专属文案 |
| `frontend/src/i18n/locales/zh-Hans/accounts.json` | E02/E03：适用翻译；排除互联专属文案 |
| `frontend/src/i18n/locales/zh-Hans/groups.json` | E02/E03：适用翻译；排除互联专属文案 |
| `frontend/src/i18n/locales/zh-Hans/settings.json` | E02/E03：适用翻译；排除互联专属文案 |
| `frontend/src/i18n/locales/zh-Hant/accounts.json` | E02/E03：适用翻译；排除互联专属文案 |
| `frontend/src/i18n/locales/zh-Hant/groups.json` | E02/E03：适用翻译；排除互联专属文案 |
| `frontend/src/i18n/locales/zh-Hant/settings.json` | E02/E03：适用翻译；排除互联专属文案 |
| `frontend/src/views/accounts/components/AccountSettingsFields.vue` | E03：复用既有模型编辑器，统一文案并验证创建、手填及图片快捷项 |
| `frontend/src/views/accounts/components/AccountTableActions.vue` | 排除互联专属入口；核对现有原生额度刷新不受影响 |
| `frontend/src/views/accounts/components/account-quota-panel/index.vue` | 排除互联专属入口；核对现有原生额度刷新不受影响 |
| `frontend/src/views/accounts/composables/useAccountSettingsFormRules.ts` | E03：复用既有模型编辑器，统一文案并验证创建、手填及图片快捷项 |
| `frontend/src/views/accounts/index.vue` | 排除互联专属入口；核对现有原生额度刷新不受影响 |
| `frontend/src/views/groups/components/AccountGroupFormModal.vue` | E03：复用既有模型编辑器，统一文案并验证创建、手填及图片快捷项 |
| `frontend/src/views/key-usage/components/KeyUsageRecords.vue` | E05：实际共享用量表格；不存在的独立页面不新增 |
| `frontend/src/views/proxies/components/ProxyBatchResults.vue` | E07：本地代理批量创建/测试及通知 |
| `frontend/src/views/proxies/composables/useProxyBatchTesting.ts` | E07：本地代理批量创建/测试及通知 |
| `frontend/src/views/proxies/utils/batch.ts` | E07：本地代理批量创建/测试及通知 |
| `frontend/src/views/settings/components/RequestCountryCard.vue` | E02：已有独立卡片，补说明与翻译 |
| `frontend/src/views/settings/components/RequestLocationCard.vue` | E02：已有独立卡片，补说明与翻译 |
| `frontend/src/views/settings/index.vue` | E02：已有独立卡片，补说明与翻译 |
| `frontend/src/views/usage/components/UsageCostCard.vue` | E06：本地费用图表 |
| `frontend/src/views/usage/composables/useUsageRecordColumns.ts` | E05：实际共享用量表格；不存在的独立页面不新增 |
| `frontend/src/views/usage/index.vue` | E05：实际共享用量表格；不存在的独立页面不新增 |
| `modules/ui` | E04：已核对 UI 原始 diff 并验证本地 BaseTable；不导入未使用依赖 |
| `release/notes.md` | E10：自有版本、说明；不复制官方发行源 |
| `release/version.yaml` | E10：自有版本、说明；不复制官方发行源 |
