# 官方 v3.51.0 原生更新合入记录

## 固定范围

- 上次官方基线：`cc9600467af55cbcf31a1fa128745c56f2c22e4c`，见
  [v3.50.0 记录](upstream-v3500-native-parity.md)。
- 本次官方目标：`89b6d2256f7d28a77e59925594a08d6dc611a523`；
  v3.51.0 标签对应 `ba2afe67d04755a247a9d534115ac168d4db7478`。
- 本地起点：`c73041275db7f2f21862397bdc546228898a74d7`，保留已经提交的
  重登入池名称修复；父提交为 NewCPR `a04f322c9b9e9395cace5b0565bd60d87ebb9d48`。
- UI 固定范围：`c5e268e948ec74bf34ae7eaed9504af800c38ec3` →
  `8ffe8a2386603961f48e7fb12ba03be7a81539ec`（v0.12.0）。
- 官方历史已改写，GitHub compare 的 merge-base 结果为 79 个文件；直接比较上述
  两端文件树实际为 65 个文件。以文件树和子模块实际差异审查，不回灌历史重写前的重复内容。
- 本轮仅本地合入与验证，未检查或修改服务器；部署基线仍须在发布时单独核对。

## 作用代码及接入

| 官方来源 | 本地实际调用路径 | 结果与必要适配 |
| --- | --- | --- |
| `976359ad2686`：子线程模型限制超时说明 | `credential/selector/capacity_wait.rs` 的 strict follow-only 等待 → `CredentialSelectionError::IneligibleSessionAccountTimeout` → `provider/observation.rs` → `GatewayError` / API | 按官方条件和原文投影。只处理根账号不在候选集合的超时；授权内模型拒绝给出具体原因，其他原因使用通用说明。保留本地 Redis 等待端口、租约释放、截止、禁止重试和三种亲和策略，不移入官方进程队列 |
| `832858dadea5`：历史额度共享解析 | `ProviderAdmin::quota_forecast_observation` → `history_window` → `credential::observed_quota_window` → `merge_passive_quota` / `snapshot::quota_windows` | 直接采用官方函数及调用关系；按本地已有模块路径放置，不新增插件接口。只解析本次响应，排除具名桶镜像；窗口 key/source/role/duration 均须匹配 |
| 同一额度提交：快照共用窗口解析 | `parse_account_quota_snapshot` → `quota_windows` | 将官方抽取函数原样接入现有额度快照，原排序、额度恢复和持久化不变 |
| `4bc59f32a181` 与 UI `2418732a2731`：共享菜单 | `BaseDropdown` / `BaseMenuItem` → `RowActionMenu` → 账号、API Key、代理列表 | 采用官方键盘与焦点算法；`reference` 适配本地 `trigger` 插槽，Popover 暴露触发元素 focus，保留本地样式、手机尺寸和所有原有动作。代理菜单接入实际 `views/proxies/index.vue` |
| UI v0.12：按下状态 | `BaseIconButton` | 默认 undefined，显式 false 保留为 `aria-pressed="false"` |
| `4bc59f32a181`：额度设置 | `AccountQuotaPanel/QuotaSettings.vue` → 既有批量保存及额度重读 API | 预热开关与标题同行；按官方使用 `validateEvent=false` 和完整提交校验，替换旧的空 trigger 规则；同步三种语言文案 |

后端模块路径均以 `backend/crates/providers/openai/src/` 为根。前端组件仍使用本地内置
Base 体系，未引入没有实际消费者的 UI npm 依赖。没有数据库 migration 或 Rust 依赖变动。

## 明确不接入的部分

| 官方差异 | 依据 |
| --- | --- |
| `4d4c1e48` 插件调度 v4、AccountScheduleOwnership 及 OpenAI/xAI 插件选号裁决 | 用户排除插件运行时；逐项核对后均用于插件显式选号及策略归属传递，不在本地原生调度新增无消费者接口 |
| `832858dadea5` 插件分页历史额度查询、SDK、Store 查询接口 | 插件查询入口排除；同提交公共原生额度解析已按上表接入，不能整包漏掉 |
| `load_account_record` 完整记录改摘要读取 | 官方消费者为插件历史查询及 peer 额度刷新；本地没有对应方法及消费者，不为此新增空接口 |
| `9f873f5a` 独立 Key 用量页宽度 | 本地没有该独立页面，沿用 [v3.43.1 记录](upstream-v3431-native-parity.md) 的边界；不因样式补丁新建产品入口 |
| 官方版本号、发布通道及文档中的插件描述 | 继续使用 NewCPR 发布体系，不冒用官方发行资产 |
| 上游 API Key、CPR 互联、PRISM、BPS/Excel 双凭据实验 | 仍为既有排除项，不由本次更新重新接入 |

NewCPR 的账号重登、Excel 入口、账号名称与配置保留、出口和手机卡片逻辑保持现有合同。
这不是整个官方服务的字节一致复制；保留差异主要是 Redis 等待实现和本地组件接入。

## 验证与旧测试更新

- 先用新增断言验证旧生产代码：严格模型受限仍只返回通用队列超时，窗口 key 不匹配
  仍被旧预测代码接纳，两个回归均确实失败。合入后对应回归通过。
- Provider 亲和相关 46 项、额度相关 66 项通过；包含严格/优先/宽松、等待开关、
  根迁移、队满、超时、凭据失效、取消及额度恢复。两组筛选有交叉，不累计为独立用例数。
- API `provider_queue_errors_preserve_the_full_http_contract` 验证明确原因贯穿正文，
  同时保留 429、`concurrency_queue_timeout` 与 `rate_limit_error`；普通 503 容量合同也通过。
- 前端受影响 Node 回归 41 项通过；类型检查、生产构建及定向 ESLint 通过；
  Rust 格式检查及 Provider all-targets Clippy（`-D warnings`）通过。
- 真实 Chromium、合成 API：菜单方向键/Home/End、Enter/Space、Escape/Tab、跳过禁用项、
  全禁用菜单和焦点返回；账号立即重登/Excel及Key重置事件；显式 pressed=false；
  额度编辑不提前校验、提交拦截错误、修正清错及正确保存负载，1440/390/320px 无遮挡。
- 账号移动卡片既有回归验证明暗主题、320/390/430/767px、多分组无重叠、跨视口选择和展开
  状态、刷新额度与排序；代理既有回归验证三种宽度的新建、测试、地区、revision、禁用、
  取消、启用及绑定删除。
- 账号质量既有浏览器回归通过单个/批量开关、部分失败、读取失败、忙时保护、首次配置、
  删除及三种宽度的明暗主题。使用合成 API，没有真实账号操作或外部请求。
- 菜单角色改为官方 `menu/menuitem` 后，同步更新受影响旧测试定位，保留原业务断言。
  代理夹具补齐既有更新详情读取；质量测试按现有批量菜单定位删除操作，不改产品逻辑迎合旧测试。
- 本轮未执行服务器、真实上游、完整 PostgreSQL/Redis 应用联调。Provider 测试使用模拟
  Store/lease 端口，不称为真实 Redis 验证；本轮未修改 Redis 脚本或数据库 schema。
- 一次性预览、截图、调试及构建产物留在仓库外；没有新增永久调试脚本或重复全量门禁。
- 已用临时独立 Git index 扫描本轮完整待提交内容，隐私检查通过；正式 index 保持未暂存。

相关合同在原账号容量等待、额度完整性/预测、前端组件及账号管理规范内更新；共同合入规则
补充历史改写时必须比较固定文件树的要求。提交、推送、合并及部署状态另按用户授权记录。
