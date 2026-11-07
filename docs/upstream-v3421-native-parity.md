# 官方 v3.42.1 接入与遗留差异补齐

## 固定基线

核对日期：2026-11-07。本记录是本轮证据；以后合并遵循
[统一合并清单](upstream-integration-checklist.md)，不能拿历史结论限制新版代码。

| 基线 | 完整提交 |
| --- | --- |
| 上次合入的官方版本 | `81de0f533136697d52565d842ccb09cc3baa1759` |
| 本次官方目标 v3.42.1 | `f772bda57b62842e7bfca0d385d9fd6099a168b3` |
| 本地起点 | `117df6a6c77c732ec53a3254dd81b87d0bd56fa5` |
| 调查时已部署源码 | `d3eac53772195325f5a6861a46ab351574a8677f` |

本轮授权范围为修改、提交、推送和合并。没有发版、打标签或生产部署。
官方 UI 基线为 `zyycn/codex-proxy-ui` v0.8.1；本地使用实际 Base 组件接入，
不能把不存在的子模块版本号变化当作完成。

## 新增八个官方提交

| 官方提交 | 实际接入或排除 |
| --- | --- |
| `ca5901ba56` OAuth pending 代理容量 | 移植 `PendingProviderMutation` 的代理容量序列化/还原与校验；真实 PG 授权提交覆盖往返、地址/容量变更、停用拒绝。 |
| `11b64b3026` main 发布通道 | 移植 Host 的 main `published` 读取、通道解析、缓存和失败处理；官方发布器校验真实公开资产后推进通道，接入本地 release workflow。 |
| `6230d95729` UI v0.8.1 | 接入浮层离场位置冻结、翻转方向动画、独立阴影、分段按钮高度/真实滑块定位、Markdown 隔离渲染与代码复制；同步真实调用点的重复 title。 |
| `750dd32200` 换绑日志 | 队列失败后的重新准入，只有绑定 CAS 成功且新旧账号不同才记录 `account.binding.migrated`；失败和普通等待不伪造迁移。 |
| `db779a4a21` 隐藏未分类专用模型 | 不适用：官方作用于上游 API Key 普通 ID 目录；本地已批准排除此产品，OAuth/Excel 走完整 native catalog。没有给 native 全局增加过滤，也没有加入无调用者代码。 |
| `39c8a6352d` 插件外链确认 | 按原有插件运行时/管理页排除范围不引入；并非漏接普通管理页面的外链。 |
| `e0a3dc20dd` 准备版本 | 保留 NewCPR 自有发行版本与说明，不复制官方品牌、包、发行技能和未经本地验证的版本宣告。 |
| `f772bda57b` 发布版本 | 添加空 `published` 表；仅部署标签不冒充有完整下载资产的正式发行。本轮不发布本地新版本。 |

API Key 产品排除的既有依据见 [v3.42.0 接入范围](upstream-v3420-native-parity.md)。

## 上次接入缺口及旧适配修正

| 项目 | 本轮真实路径与结果 | 归因 |
| --- | --- | --- |
| 队满/超时被报成所有账号忙 | OpenAI observation、xAI failure → `ProviderErrorKind` → Core retained/retry/circuit 分类 → API HTTP 429，分别为 `concurrency_queue_full` / `concurrency_queue_timeout`。TokenExpired、存储故障、无候选不机械改成队满。 | 旧官方已具有的错误链，本地未接齐。 |
| 排队预算耗尽额外禁止恢复 | Core 按官方移除额外等待预算门禁；补齐发送状态、换号次数、required owner 和 retry prohibition。OpenAI 允许立即空闲准入，但不能再开启一个等待窗口。Native 恢复不把队列错误当作可重放原因。 | 遗留适配限制；不等于已证明它触发原生产事件。 |
| Redis 保留池和总等待量 | 实际 Redis enqueue 接入 Reserved/High 免普通单账号人数上限；每 Provider、每容量池、每进程最多 1024 个等待者。晋升、取消、错误归还名额，保留跨实例原子执行租约和 FIFO。 | 公共队列已有规则，但本地实际 Redis 路径未接齐。 |
| 重扫提前耗尽 | 真实账号事实/凭据冲突最多重试三次，含首次共四次，终态 `AccountSnapshotChanged`；容量 publication、缓存、亲和重查不消耗该计数，继续受请求取消/截止和已开启等待窗口限制。 | 本地外层“三轮总扫描”过严，不是官方所有重查的上限。 |
| 同窗口额度恢复 | 采用官方“reset 未前进且未触顶”的观察条件，允许没有 used_percent 的已知窗口；继续要求两次新鲜观察。 | 本地 reset 严格相等及百分比必填的额外限制。 |
| 更新源缺失 | 原 `update-index` 不存在；读取本仓库 main 的公开通道表，公开资产未齐全时不宣告可更新。 | 本地发行接线缺口，同时采用官方新的索引实现。 |

## 保留的实际差异

- **Redis 所有权**：官方进程内队列规则接入已有 Redis owner；没有将 1024 扩成集群全局限制，
  也没有用内存计数替代跨实例执行容量。该上限归进程中的 coordinator 队列实例所有，
  生产 bundle 构造一次；测试用独立 coordinator 模拟另一实例。4096 仍只是取消清理 channel 的大小。
- **独立额度时钟**：保留本地防迟到写入、重复和乱序观察保护，不将它混同于已删除的恢复条件。
- **旧上游 401 的终态交付**：保留现有 NewAPI 兼容合同。某个账号曾报 401，后续本地池已耗尽时，
  返回当前池错误；不将旧账号认证错误继续投射成下游 Key 认证错误。其他保留终态按原合同。
  此处与官方保留旧错误不同，不能宣称整个 coordinator 字节相同。
- **受保护 main 的发行写入**：官方通道计算和资产验证不变。NewCPR 不能直接 PUT main：
  从精确 main 暂存索引提交，显式触发现有 `Security Scan` 的 `privacy_only`，成功后非强制快进。
  未通过、超时或 main 并发前进则停止；不关闭保护或伪造检查。GITHUB_TOKEN 自动提交不触发普通
  push workflow，因此必须显式 dispatch。只给发布及索引恢复 job 所需的 `actions: write`。
- **前端接线**：本地 Base 组件、主题命名空间、i18n 与既有表单属性保留；官方浮层与 Markdown
  的实际实现接到本地消费者。未引入完整 UI 子模块或插件依赖。分段按钮此前落后的尺寸跟踪和
  键盘事件引用也一并按官方当前实现补齐。
- **既有产品合同**：账号身份、出口、Excel、State、冻结迁移及已取消功能边界保持；本轮无数据库迁移。

## 已知官方机制与验证边界

严格子线程可等待根绑定恢复/迁移，FIFO 不允许后来的普通请求越过队首。
因此执行槽空闲和等待队列满仍可能同时存在。此次状态码与恢复接线对齐不等于消除队首阻塞，
没有擅自引入另一套调度算法。严格根请求的队列失败换号仍服从已有设置、归属和换号预算；
成功准入后才迁移绑定。

验证使用隔离本机 PG/Redis、合成请求和浏览器组件页，未向生产发送测试流量。
真实 NewAPI 收到新 429 后的运行策略、生产长时间压力、真实官方账号网络、正式 GitHub
多平台发行及受保护主线索引发布均未在本轮执行；不能据此承诺零缺陷。

## 验证与规则同步

- 7 个区分新旧行为的用例验证旧行为会失败：OAuth 暂存及真实授权提交、Redis 总等待量/Reserved、
  等待耗尽后立即准入、三次快照冲突重试、无百分比的同窗口额度恢复。Selector 复现只恢复两段旧门禁，
  避免用接口不兼容的编译失败冒充行为证据；验证后逐字节恢复源码，临时探针不提交。
- Core coordinator 定向回归 134 项；Admin pending 8 项；API 错误链 33 项；Host 更新链 61 项。
- OpenAI 最终整包 1234 项通过，包含 credential/selector/quota、严格亲和、图片、Live、原生续写和传输；
  xAI selector 37 项和 Provider 63 项。
- 真实 Redis capacity_wait 30 项；真实 PG authorization 2 项，均设置实际隔离后端，未把无环境跳过算通过。
- Ruby 3.4 下 publication 14 项 / 113 断言，publish 6 项 / 120 断言，platforms 7 项 / 223 断言；覆盖资产先验、精确 SHA、
  其他发行线保留、版本准备保留 published、检查失败及并发拒绝。
- Workspace 全目标编译检查、App 架构 14 项通过；前端 Node 测试 1155 项及最终受影响 25 项通过，
  ESLint、vue-tsc、Vite 构建通过。Actionlint 使用现有 CI 对旧工具 runner/queue 识别能力的精确兼容参数，
  没有扩大忽略范围；release platform 回归验证实际字段。
  浏览器确认上下翻转、键盘切换、明暗主题、Markdown 复制反馈及滑块真实位置；没有把复制提示
  当作系统剪贴板内容的独立验证。临时页面和截图不提交。
- 现有 `AGENTS.md` 指向统一合并清单；更新容量等待、额度观察、合并/发行规范。
  淘汰旧 503 映射、等待到期禁止恢复、所有重扫共三轮、保留池普通人数限制和 update-index 指针条款。
  旧 Redis 故障代理同步新的 Lua 参数数目，继续验证真实丢包/取消；图片、Live、原生续写和严格模型限制等待的旧分类断言也同步为 429 对应种类，保持归属与 retry prohibition 断言。
- 保留必要行为回归；没有提交调查探针、生产日志、临时运行环境或新增一套合并流程。

## 官方增量路径核对

下表覆盖该固定官方增量的全部 69 个路径；共享组件变化还沿本地真实消费者检查。

| 官方路径 | 本地落点或处理 |
| --- | --- |
| `.agents/skills/cpr-release/SKILL.md` | 归入既有 `deploy/README.md` 与合并/发行 spec；不引入官方专属技能和插件工具。 |
| `.agents/skills/cpr-release/references/experimental.md` | 归入既有 `deploy/README.md` 与合并/发行 spec；不引入官方专属技能和插件工具。 |
| `.agents/skills/cpr-release/references/operations.md` | 归入既有 `deploy/README.md` 与合并/发行 spec；不引入官方专属技能和插件工具。 |
| `.agents/skills/cpr-release/references/preparation.md` | 归入既有 `deploy/README.md` 与合并/发行 spec；不引入官方专属技能和插件工具。 |
| `.github/scripts/published-channels.rb` | `release/published-channels.rb`；官方通道/资产代码及受保护 main 接入。 |
| `.github/workflows/release.yml` | 同路径；check/publish、index_only 与精确索引提交检查接线。 |
| `backend/crates/gateway-admin/src/model/provider_credentials.rs` | 同路径原生移植并同步实际测试入口。 |
| `backend/crates/gateway-admin/tests/model/provider_credentials.rs` | 同路径原生移植并同步实际测试入口。 |
| `backend/crates/gateway-host/src/system_update/metadata.rs` | 同路径原生移植并同步实际测试入口。 |
| `backend/crates/gateway-host/tests/system_update/channels.rs` | 同路径原生移植并同步实际测试入口。 |
| `backend/crates/gateway-host/tests/system_update/discovery.rs` | 同路径原生移植并同步实际测试入口。 |
| `backend/crates/gateway-host/tests/system_update/download.rs` | 本地 `system_update/mod.rs` 内对应下载用例，指向 main 的通道表 fixture。 |
| `backend/crates/gateway-host/tests/system_update/mod.rs` | 同路径原生移植并同步实际测试入口。 |
| `backend/crates/gateway-plugin/sdk/docs/capabilities.md` | 既有插件产品排除；本轮不引入该专属路径。 |
| `backend/crates/gateway-store/tests/postgres/provider_accounts/authorization.rs` | `backend/crates/gateway-store/tests/postgres/provider_accounts/mod.rs` 中真实授权提交用例。 |
| `backend/crates/providers/openai/src/credential/catalog.rs` | 普通 ID/API Key 上游目录不在本地产品范围；native catalog 不增加专用模型过滤。 |
| `backend/crates/providers/openai/src/credential/selector.rs` | 本地 `selector/mod.rs` 与 `selector/admission.rs` 的实际绑定 CAS 成功分支。 |
| `backend/crates/providers/openai/tests/credential/catalog.rs` | 普通 ID/API Key 上游目录不在本地产品范围；native catalog 不增加专用模型过滤。 |
| `backend/crates/providers/openai/tests/credential/contract/scheduling_projection.rs` | 同路径；记录真实迁移成功事件及原因、前后账号。 |
| `deploy/README.md` | 同路径更新当前运行合同；主题变量使用本地 `cp` 命名空间。 |
| `docs/api.md` | 接入换绑事件/队列错误合同；API Key、插件专属新增说明按既有范围排除。 |
| `docs/architecture.md` | 同路径更新当前运行合同；主题变量使用本地 `cp` 命名空间。 |
| `docs/plugins.md` | 既有插件产品排除；本轮不引入该专属路径。 |
| `docs/theme.md` | 同路径更新当前运行合同；主题变量使用本地 `cp` 命名空间。 |
| `frontend/package.json` | 没有本地 UI 子模块依赖；改实际 Base 组件、主题与消费者，不添加无效版本引用。 |
| `frontend/pnpm-lock.yaml` | 没有本地 UI 子模块依赖；改实际 Base 组件、主题与消费者，不添加无效版本引用。 |
| `frontend/src/components/AccountGroupMarks.vue` | 同路径核对消费者；移除重复 title/改 aria-label。Modal/Card 的业务标题、截断值提示保留。 |
| `frontend/src/components/ClientImportModal.vue` | 同路径核对消费者；移除重复 title/改 aria-label。Modal/Card 的业务标题、截断值提示保留。 |
| `frontend/src/components/RequestLocationDetectButton.vue` | 本地无此独立组件；核对对应表单/设置及公共 BaseIconButton，未保留官方要删除的 title override。 |
| `frontend/src/components/schema-form/index.vue` | 本地无此独立组件；核对对应表单/设置及公共 BaseIconButton，未保留官方要删除的 title override。 |
| `frontend/src/i18n/locales/en/accounts.json` | 删除无调用者的旧 capacity.viewDetails；保留本地其他详情说明。 |
| `frontend/src/i18n/locales/en/plugins.json` | 既有插件产品排除；本轮不引入该专属路径。 |
| `frontend/src/i18n/locales/zh-Hans/accounts.json` | 删除无调用者的旧 capacity.viewDetails；保留本地其他详情说明。 |
| `frontend/src/i18n/locales/zh-Hans/plugins.json` | 既有插件产品排除；本轮不引入该专属路径。 |
| `frontend/src/i18n/locales/zh-Hant/accounts.json` | 删除无调用者的旧 capacity.viewDetails；保留本地其他详情说明。 |
| `frontend/src/i18n/locales/zh-Hant/plugins.json` | 既有插件产品排除；本轮不引入该专属路径。 |
| `frontend/src/layout/components/AppSidebar.vue` | 同路径核对消费者；移除重复 title/改 aria-label。Modal/Card 的业务标题、截断值提示保留。 |
| `frontend/src/views/accounts/components/AccountCapacityIndicator.vue` | 本地对应 `AccountCapacityCell.vue` 与 BaseIconButton 已核对；同步实际重复提示，保留业务标题。 |
| `frontend/src/views/accounts/components/AccountModelAccessField.vue` | 同路径核对消费者；移除重复 title/改 aria-label。Modal/Card 的业务标题、截断值提示保留。 |
| `frontend/src/views/accounts/components/account-create-modal/AccountOAuthFields.vue` | 本地对应 `AccountCreateModal/AccountOAuthFields.vue` 与 BaseIconButton 已核对；同步实际重复提示，保留业务标题。 |
| `frontend/src/views/accounts/components/account-import-tasks/index.vue` | 本地 `AccountImportTasks/index.vue` 无对应重复帮助 title，公共 BaseIconButton 已接入。 |
| `frontend/src/views/accounts/components/account-quota-forecast-modal/index.vue` | 本地 `AccountQuotaForecastModal/index.vue` 无对应重复帮助 title，公共 BaseIconButton 已接入。 |
| `frontend/src/views/accounts/components/account-quota-panel/ResetCreditAutomation.vue` | 本地对应 `AccountQuotaPanel/ResetCredits.vue` 与 BaseIconButton 已核对；同步实际重复提示，保留业务标题。 |
| `frontend/src/views/login/components/LoginThemeToggle.vue` | 本地对应 `login/components/LoginPanel.vue` 与 BaseIconButton 已核对；同步实际重复提示，保留业务标题。 |
| `frontend/src/views/plugins/components/PluginExternalLinkDialog.vue` | 既有插件产品排除；本轮不引入该专属路径。 |
| `frontend/src/views/plugins/components/PluginHelpPopover.vue` | 既有插件产品排除；本轮不引入该专属路径。 |
| `frontend/src/views/plugins/components/PluginManagementFrame.vue` | 既有插件产品排除；本轮不引入该专属路径。 |
| `frontend/src/views/plugins/composables/usePluginFrameSession.ts` | 既有插件产品排除；本轮不引入该专属路径。 |
| `frontend/src/views/plugins/management/bridge-contract.ts` | 既有插件产品排除；本轮不引入该专属路径。 |
| `frontend/src/views/plugins/management/bridge-runtime.ts` | 既有插件产品排除；本轮不引入该专属路径。 |
| `frontend/src/views/proxies/components/ProxyBatchCreateFields.vue` | 本地对应 `proxies/components/ProxyBatchCreateModal.vue` 与 BaseIconButton 已核对；同步实际重复提示，保留业务标题。 |
| `frontend/src/views/settings/components/AccountAffinityCard.vue` | 本地无此独立组件；核对对应表单/设置及公共 BaseIconButton，未保留官方要删除的 title override。 |
| `frontend/src/views/settings/components/AccountWarmupCard.vue` | 同路径核对消费者；移除重复 title/改 aria-label。Modal/Card 的业务标题、截断值提示保留。 |
| `frontend/src/views/settings/components/ConcurrencyPolicyCard.vue` | 本地无此独立组件；核对对应表单/设置及公共 BaseIconButton，未保留官方要删除的 title override。 |
| `frontend/src/views/settings/components/SmartSchedulingModal.vue` | 本地无此独立组件；核对对应表单/设置及公共 BaseIconButton，未保留官方要删除的 title override。 |
| `frontend/src/views/settings/components/backup/BackupRecordsCard.vue` | 同路径核对消费者；移除重复 title/改 aria-label。Modal/Card 的业务标题、截断值提示保留。 |
| `frontend/src/views/settings/components/client-version/components/ClientVersionPolicyCard.vue` | 同路径核对消费者；移除重复 title/改 aria-label。Modal/Card 的业务标题、截断值提示保留。 |
| `frontend/src/views/settings/components/error-protection/AccountErrorProtectionCard.vue` | 同路径核对消费者；移除重复 title/改 aria-label。Modal/Card 的业务标题、截断值提示保留。 |
| `frontend/src/views/settings/components/privacy/PrivacyPolicyCard.vue` | 本地无此独立组件；核对对应表单/设置及公共 BaseIconButton，未保留官方要删除的 title override。 |
| `frontend/src/views/settings/components/privacy/StepFields.vue` | 本地无此独立组件；核对对应表单/设置及公共 BaseIconButton，未保留官方要删除的 title override。 |
| `frontend/src/views/settings/components/privacy/StepList.vue` | 本地无此独立组件；核对对应表单/设置及公共 BaseIconButton，未保留官方要删除的 title override。 |
| `frontend/src/views/settings/pricing/PricingToolbar.vue` | 本地无此独立组件；核对对应表单/设置及公共 BaseIconButton，未保留官方要删除的 title override。 |
| `frontend/src/views/settings/pricing/PricingUnit.vue` | 本地无此独立组件；核对对应表单/设置及公共 BaseIconButton，未保留官方要删除的 title override。 |
| `frontend/src/views/theme/components/ThemePreviewCanvas.vue` | 同路径核对消费者；移除重复 title/改 aria-label。Modal/Card 的业务标题、截断值提示保留。 |
| `frontend/src/views/usage/components/DiagnosticsDownloadButton.vue` | 本地无此独立组件；核对对应表单/设置及公共 BaseIconButton，未保留官方要删除的 title override。 |
| `modules/ui` | 没有本地 UI 子模块依赖；改实际 Base 组件、主题与消费者，不添加无效版本引用。 |
| `release/notes.md` | 保留 NewCPR 自有版本和发行说明；version.yaml 增加空 published，不假冒完整发行。 |
| `release/publish` | 本地已使用 prepare 提交及仅替换 version 行；新增保留 published 的真实 Git 发布回归。 |
| `release/version.yaml` | 保留 NewCPR 自有版本和发行说明；version.yaml 增加空 published，不假冒完整发行。 |


UI v0.8.0 → v0.8.1 的四个提交分别为表格排序说明简化、Markdown 代码复制、浮层/分段交互修正及版本发布。
`Popover/usePopoverPosition`、`Select/useSelect/useSelectPosition` 在本地对应单文件 BasePopover/BaseSelect；
`IconButton`、`Segmented`、`Table` 对应同名 Base 组件。Markdown renderer、样式和复制按钮接到 SystemUpdateModal，
旧无消费者的 `SystemUpdateModal/markdown.ts` 删除。主题类型/派生/Tailwind/动画均有实际消费者。
本地没有官方的列拖动说明字段，复制反馈复用三种语言已有 copy/copied/copy-error 文案；
UI playground、安装文档和官方包版本不成为本地运行依赖。
