# 官方 v3.47.0 合入记录

## 固定范围

- 本地起点：`7c9acb2a57329c17fccd79f133600710d2619c13`（`Heykode/newcpr`）。
- 上次官方：`28ad8fb1c46efbbb2d2678e9bc85205f480e3b2f`（v3.46.0）。
- 本次官方：`e1f84c210b5bc7423cec011a65e0fc2310e24aad`（v3.47.0）。
- UI：`45b076735d67f973588d93144078c7605cf1ade7` →
  `68458e62e7fb9fadbacea75ce48522f2ba85b8b2`（v0.11.0）。
- 本记录随本批代码、规范及回归一起提交；版本发布和部署不属于本批范围。
  验证期间没有访问生产，未将本地起点当作线上源码验证。

## 提交与实际接入

| 官方提交 | 官方变化 | 本地接入与状态 |
| --- | --- | --- |
| `bab781cb` | v3.46.0 发布通道 | 发行仓库专属元数据，不复制官方资产为 NewCPR 资产 |
| `2cae28d7` | 主题预设底色；UI v0.10.1 | 接入 `theme/core/constants.ts`、`resolve.ts`、`derive/colors.ts`；底色只参与表面派生，不改种子，显式背景优先 |
| `3bd04218` | 刷新期间保留账号目录排除 | 接入 OpenAI catalog、Core omissions、PostgreSQL 查询及实际 Provider 选号回归；删除 30 分钟资格截止和通知即失效 |
| `9b4a042b` | 多条独立预热 Cron；UI v0.11.0 | Core policy → Store → Worker；API PATCH 校验、前端表单和真实标签输入同时接入 |
| `1eac929e` | 统计投影减少临时写盘、事务工作内存配置 | **未合入**；缺少完整 analytics worker 前置架构，迁移计划见下文 |
| `0e02a056` | 官方版本 v3.47.0 | 来源版本记录；不提前改写本地发布版本或说明 |
| `e1f84c21` | v3.47.0 发布通道 | 保留 NewCPR 自己的发行源和受保护主线发布流程 |

两项后端变化分别是官方旧版缺陷的后续修复和新增功能，不归因为上次漏合。

## 调用链与必要差异

### 账号目录

`CodexCredentialCatalogService::restore/refresh_catalogs` 发布账号 scope 快照；
`account_can_request` 和 `account_catalog_omissions` 服务即时、等待选号；大批遗漏通过
`AccountCatalogOmissions` 进入 `PgProviderAccountRepository::list_catalog_candidates`。

- 移除 `AccountEntry.invalidated`、`AccountCatalogOmissions.observed_after` 和 SQL 时间过滤。
  `CATALOG_REFRESH_AFTER` 只控制后台刷新，不能解除排除。
- ETag 通知仍进入 pending；只有该账号更新的观测确认 ETag 才移除，发布其他账号不误确认。
  周期刷新失败可返回最近成功快照；有未完成 ETag 通知时沿既有错误/退避链处理。
- 账号 ID、凭据 revision、套餐 scope 及显式白名单合同保留；缺少同 scope 证据时交给上游验证。
- 本地 Excel 保留原生 `native_ids`、marker 文档区分及混合目录内存筛选。理由是合成模型不能
  冒充原生账号权限；混合 hash 不能错误下推为原生文档。保留出口、IPv6、画像和安装身份合同。
- 不修改亲和、错误换号次数、队列上限、冷却时间或根/子线程归属规则。

### 预热

官方 `provider_ports/runtime_policy.rs` 在本地的 owner 为 `provider_ports/mod.rs`。
直接迁入解析/校验/匹配实现，持久字符串按 `;` 分成 `Vec<cron::Schedule>`，任一规则命中即可。
保留部署时区、Worker、持久分钟 claim 和空闲账号条件。

本地 `gateway-api/admin/settings.rs` 的部分更新还存在额外单条校验，现复用 Core 校验；
随后组合完整设置时仍检查启用与模型。前端 `useSettingsForm` 的额外提交校验也同步更换，
避免界面能输入但保存被旧规则拒绝。两处前端校验共用 `utils/warmup-schedule.ts`，
后端仍负责具体 Cron 取值/日历合法性，不增加新的限制。

总长度 255 UTF-8 字节；旧单条配置可直接读写，无新增迁移、无配置清空。
重叠规则同一分钟只 claim 一次，`01:30`、`08:45` 不产生 `01:45`、`08:30`。

### UI v0.11.0

官方新增 InputTag、TagOverflow、useTagFit 及更新后的 Select/SelectTags/useSelect/position、
Input、Textarea、共享 field/clear-button、主题 inputTagBg 代码落入现有 Base 组件及 `controls/`。
新增 `@vueuse/integrations`、`sortablejs` 和类型依赖，实际用于标签拖拽。

必要接入：

- CSS `z` → `cp`，尺寸 `small/default/large` → `sm/md/lg`，沿本地主题类型、持久键和表单注入。
  标签 `type: default` 保留，不和中号尺寸混淆。
- Select 完整保留字符串/数字、单/多选、清空、搜索、remote、键盘及折叠能力。
  单选清空仍为官方 `undefined`；既有页面在消费端映射为空筛选或忽略其不支持的清空。
  不为旧调用者删除官方类型能力。
- `BasePopover` 增加默认 true 的 `teleported`，标签浮层设为 false，官方 reference 插槽接
  本地 trigger；`.cp-popover` 支持选择器识别内部弹层。
- `BaseNumberInput` 接官方 field geometry 与三种尺寸；保留本地必填 number、label、即时更新及
  默认紧凑尺寸，现有策略编辑依赖这些接口。没有改成官方可空数字草稿 API。
- 自动填充动画统一 `cp` 名称，专属字段选择器保留命名空间，动画样式不使用 Vue scoped 后缀，
  确保浏览器 `animationstart.animationName` 与监听匹配。移植初稿未同步表单值的失败已复现，
  修正的是组件接入，不改变请求或账号限制。
- Table 去除单元格背景过渡；输入族默认宽度按官方组件层设置，允许调用方宽度覆盖。
  本地没有 DatePicker 组件，官方该组件两处共享样式接入没有对应 owner，不新增无消费者控件。
- UI 仓库 playground、文档站、演示页面与发布元数据不是 NewCPR 产品入口，不复制。
  账号管理多分组文字布局保留，并以实际桌面/手机布局验证。

## 统计两项的完整迁移计划（未实施）

官方 `1eac929e` 的两个优化是：缩小增量投影中 `matching materialized` 的列集，降低临时写盘；
增加 `pool.analytics_projection_work_mem_mb`，只在投影/回填事务通过 transaction-local
`set_config('work_mem', ..., true)` 生效。默认 None，范围为 `1..=i32::MAX/1024` MB；
队列 ACK、整桶重建和在线查询不因此获得更高内存额度。

本地当前是 `PgObservabilityRepository` 直接 SQL 及恢复小时基线，不能直接粘贴 worker 配置，
也不能把它误列成插件专属功能。完整迁移需按以下顺序完成：

| 顺序 | 必须迁移/核对 | 验收依据 |
| --- | --- | --- |
| 1 | 对照官方迁移 0028/0030：facts、pending requests/buckets、change versions、15m/hour 汇总、build state/generation；追加本地迁移，不改已应用序号 | 升级、重启、回填、空库及历史库一致；更新 schema freeze |
| 2 | 完整 `observability/analytics/*`：投影、增量贡献、回填游标、桶重算、重置状态机、精确版本 ACK 与查询预算 | 晚到更新/删除、重复任务、并发、失败重试不丢计数或重复累计 |
| 3 | Store bundle、Host/App bootstrap 注册 worker，接 `ProviderUsageBillingProjector` | 实际部署 owner 执行投影；费用来源、真实 usage、cache/Excel 计费与账本保持一致 |
| 4 | Usage/dashboard、账号汇总、直方图/分位数、时间桶查询逐项切换；保留本地请求筛选、诊断与展示扩展 | 同一数据与旧查询对账，小时/15m 边界、未完成请求、零/未知值、费用精度均覆盖 |
| 5 | 业务备份/恢复、清理与 analytics readiness/代际协调；处理本地已恢复小时基线 | 未完成回填不导出半份历史，恢复不双计；全生命周期不能依赖两套冲突真值 |
| 6 | 最后接本次窄列投影及事务 work_mem；配置上下限与事务结束恢复测试 | 真实 PostgreSQL 对比 temp bytes、耗时、峰值内存和前台查询延迟；不以放大内存替代性能证据 |
| 7 | 在隔离数据副本完成迁移/恢复验证，再制定版本专属部署计划 | 数据迁移另行授权；不可套无迁移快速回退，不跨新 schema 自动降级镜像 |

## 验证与边界

- 修复前：实际 Provider `account_allowlists_override_catalog_omissions_but_weight_affinity_and_pinning_do_not`
  在 ETag 通知后选中不支持模型的高权重账号；官方多 Cron 分钟配对测试被单条解析拒绝。
- 修复后：Provider 型号资格 11 项、catalog 服务 23 项、Core provider ports 14 项、API settings 39 项、
  真实 PostgreSQL runtime settings 29 项、catalog/core adapter 4 项通过。包含一小时旧目录、刷新 503、
  新成功目录恢复资格、Excel 隔离和重叠 Cron 分钟 claim。
- App 架构 14 项、workspace 测试目标完整编译及 Clippy `--workspace --all-targets -- -D warnings` 通过。
- 前端全套初跑 1112 项通过；46 项因两个旧 VM loader 不认识新共享 helper 失败。
  更新 loader 后受影响测试及新增回归共 66 项通过；没有恢复旧单条校验。
- 前端 typecheck、ESLint、生产 build 通过。主题预设种子不变/显式背景优先、标签背景派生和
  旧 Cron / 多 Cron 保存重读、非法表达式及长度边界有必要回归。
- 浏览器：既有表单/弹窗、主题预览、策略数值输入、账号多分组及手机卡片已通过；
  新增多规则实际表单保存/重读、失焦提交、IME、退格、溢出删除、拖拽、数字单/多选、
  清空和禁用在 1440/390/320 像素通过。自动填充动画另以真实 CSS 动画触发验证。
- 更新现行 API、架构及 Trellis 合同，淘汰“目录过期即可放行”和“仅单条预热”的规则。
  旧浏览器 Markdown 引用更新为当前 BaseMarkdown，测试结果不要求恢复被迁走的产品文件。
- 仅使用本机隔离 PostgreSQL/Redis、模拟上游和拦截 API 的浏览器。未执行真实账号调用、生产测试、
  Linux 容器 CI、版本发布或部署；统计性能没有改动，也没有宣称改善。

隐私核对中，Core 测试文件变更使既有五条 IPv6 类别解析夹具的整文件指纹失配。
逐条复核它们与基线完全相同、仅本地解析后，补记当前文件指纹并保留历史记录；
没有新增地址许可规则、关闭检查或修改夹具行为。

临时环境和原始日志放在仓库外，不提交一次性移植/调试脚本。
