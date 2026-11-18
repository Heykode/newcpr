# 官方 v3.52.0 合入记录

上次[官方基线](upstream-v3510-native-parity.md)为 `89b6d2256f7d28a77e59925594a08d6dc611a523`，
本次固定到 `83521e82f5a9260f0d05d15f04a7eac69411e0a6`，共 6 个提交。
NewCPR 本地起点为 `7f86fb62c9b17139ac8ad7ad3950fa8fe4f5c492`；上次部署记录为 v3.51.1
（源码 `75cfcd0240`）。本次只验证本地代码。

| 官方变更 | NewCPR 实际入口与处理 |
| --- | --- |
| `bb440a3cbb60`：GPT-6.2 Astra、Sol 计价 | `providers/openai` 的 `transport/usage.rs` 采用官方两条完整价目和推理模型分支。原生响应解码、Compact、Excel 及管理端已有的费用计算继续调用 `openai_billing_breakdown`；`web_search_preview` 随 `canonical` 解码选择推理模型工具价。没有复制内置价表的管理入口，不引入对应插件接口。 |
| 同提交：速度档位、错误来源展示 | 请求明细 `UsageRecordsTable` → `UsageBillingCell`：优先使用实际 `serviceTier`，缺失才回看费用明细；Fast/Priority 单闪电，Ultrafast 双闪电。无费用时保留速度标记，不显示明细按钮。错误来源沿用本地文字、提示及详情，只去掉底色。三语补齐档位标签，保留分组设置仍使用的 `fastMode`。 |
| `84bcd0e1c0aa`：按账号历史名称或邮箱筛选 | 现有 `UsageQueryFilters` → `accountSearch` → `RequestSearchFilter.account_search` → `push_request_search`。采用官方两字段 `LIKE` 与字面量转义，移除专用筛选中对当前账号和 ID 的匹配。`usage` 和 `ops` 查询分别使用请求或事件自己的快照。通用搜索、账号选择器、ID 多选和当前备注展示保留各自合同。 |
| 同提交：汇总与分页 | `collection_for`、`merge_contributions` 和 `push_analytics_filter` 已按 `details` 选择请求级来源；原有顶部图表和列表继续共享条件。无需重命名本地 API、重复实现统计分支或迁移历史数据。 |
| `7459c16a3a32`：数据库部署要求 | 部署文档补充 PostgreSQL 最低版本 16。默认 Compose 已为 18，此项不改变镜像。 |
| `3b4fb390c72e`：picomatch 审计例外 | v3.51.1 已在 `frontend/pnpm-workspace.yaml` 限定同一个 GHSA，本次无需重复修改。 |
| `ba8745a71403`、`83521e82f5a9`：版本与发布通道 | 保留 NewCPR 自己的版本和发布通道；无官方发行元数据混入。 |

## 验证

- Rust 1.99.1。旧实现在 GPT-6.2 的价格矩阵、搜索工具估算和专用账号筛选三个用例中失败；
  移植后，`provider-openai --test main` 的 `transport::usage` 和 `transport::canonical` 共 71 项通过。
  矩阵包括 272,000 / 272,001 输入 Token、缓存读写、分项金额及不支持的 Flex 档。
- `gateway-store --test main` 的 `postgres::observability::search` 使用显式本地 PostgreSQL 16.13
  连接和隔离 schema，3 项通过；覆盖字面量及大小写、历史身份、改名和删除、分页、汇总、通用搜索，
  并检查中间重试事件与最终请求不会互借账号名称。App/API 架构及真实 URL 参数解析检查共 20 项通过。
- 前端用量及三语相关测试 34 项通过，生产构建包含 Vue 类型检查。既有浏览器测试使用本地模拟数据，
  在 1440px 和 390px 下检查账单回退、无账单时的双闪电及 Priority 单闪电、详情按钮、错误来源提示与透明背景。
  测试中的系统更新详情补齐现有接口要求的 `policy`，不改变系统更新业务代码。
- 本轮没有数据库迁移、真实模型调用或服务器验证；不追算旧费用，不调整账号调度、重登、出口或模型权限。
