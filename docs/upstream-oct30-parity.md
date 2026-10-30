# 2026-10-30 官方增量接入

## 固定范围

- 目标为 `Heykode/newcpr`，工作分支 `codex/proxy-official-parity`。
- 本地起点 `fe8e188c6`，包含此前普通代理及搜索地区对齐。
- 官方范围：`12771aca8719ed4b23b0362b23d8d81f228190ca..fdd124e72f9d690a72a130ac89127b5a8f5ecacb`。
- 本轮实现七项行为。不接入 PRISM、插件运行时、隐私规则或上游 API-key 账号。
- 保留用户此前要求删除的两项人工抓包测试；不重新添加或运行真实上游抓包。
- 本文记录源码接入，不代表提交、推送、发布或生产部署。

## 接入台账

| 官方来源 | 改前 | 改后与接入位置 |
| --- | --- | --- |
| `b3dd9b533` / #551 | 设置保存被明确拒绝后重新读取，可能覆盖未保存内容 | `useSettingsForm` 对结构化 4xx 保留草稿；不确定或服务端错误仍重新读取 |
| `0e1c609cd` / #552 | 定时备份保留策略仅扫描前 1000 份 | backup policy/task/repository 使用全局偏移分页；收集候选后才删除，单轮最多 10 份，支持扫描取消 |
| `fdd124e72` / #559 | S3 探针 GET 正文读取失败会提前退出，残留临时对象 | 失败后尝试 DELETE，再返回 getObject 阶段失败；删除失败不掩盖原读取问题 |
| `7e7acad6a` / #558 | HTTP 回退没有可用 WS 控制通道，中断请求返回通用错误 | 有效中断返回 `response.interrupt.failed`，继续交付真实模型响应；未知控制消息和非法中断仍按原错误处理 |
| `aa0ce5203` | 模型目录正文超过 1 MiB 被拒绝 | 取消正文限制；完整解析、返回；驻留目录缓存限定 32 MiB，过大结果可返回但不长期缓存 |
| `aa0ce5203` | Guardian 子请求缺少父连接的路由 Cookie 快照 | 新增官方 guardian/routing_affinity 模块；父响应按 Key、账号隔离，HTTP/WS 发送应用同端点的有效路由 Cookie，WS 池区分快照 |
| `3a2ccbc5e` / #556 | 统计概览整包计算，卡片互相等待并计算未选指标 | 健康、性能、成本独立接口和加载状态；切换指标仅刷新本卡，只有负载查询计算并发；缓存区分完整筛选和指标 |

## 官方语义与必要适配

1. Guardian 两个独立生产模块按固定官方源码复制。只复用 `__oailb`、`__cflb`，
   保留 Cookie 原始过期时间；不记录到日志或数据库，不接受歧义重复、超长或错误路径。
   父响应快照最多 4096 份、保留 30 分钟，隔离 Key 和选中账号。
   本地普通粘性选号命中不能证明 Guardian 父响应归属，因此 Guardian 无显式 owner 时
   先按未知归属清理引用，再恢复已验证父响应。普通请求的粘性调度没有修改。
   现有 Excel 不参与原生 Guardian 路由，不增加账号候选或改变代理/IPv6 出口。
2. 中断补丁遵循官方有效事件判定和失败事件形状；本地已有结算先于完成帧发送的合同保留。
   回归先等待结算开始，验证此时能拒绝中断，再释放结算，验证真实完成帧、空闲中断和后续请求。
   没有把 HTTP 请求伪装成中断成功，也没有改动计费/终态写入顺序。
3. NewCPR 使用即时 SQL 和业务备份小时基线，未采用官方另一套 analytics rollup 表和发布代次。
   单项查询在本地只读可重复读事务中完成；保留质量检测排除、历史恢复基线、完整筛选和原有费用口径。
   成本场景仍由 Provider 计价能力恢复；非成本指标不触发计价恢复。
   本地原有“调度”视图继续提供，其查询不计算并发；旧 overview 接口保留兼容。
4. 指标缓存沿用本地 2 秒新鲜度，容量 64，合并同键进行中的查询。
   完整时间、筛选和指标参与缓存键；失败不缓存为成功；等待同键查询及计费恢复均计入数据库配置的总查询时限。
   未搬入依赖 rollup 发布代次的 15 秒缓存和 60 秒过期回退，不把旧数据伪装成最新成功结果。
5. 前端保留原表格、调度图和 Key × 模型分页。切换筛选立即清空旧结果并取消旧请求；
   相同筛选的翻页失败保留上一页并显示错误，避免误认为已翻页。通用 GET 一次瞬时重试继续生效。
   原有调度视图没有请求活动计数，缺失样本保留为空，不因独立查询而画成零；浏览器直接检查图表数据。
   测试使用当前 `Codex` 展示名及正式响应信封，不修改管理员 Cookie 或认证规则来迁就官方夹具。
6. 单项接口先分离 `metric`，再交给既有 `UsageQuery` 校验。避免双层 `flatten`
   将合法的 Provider、账号等筛选误判为未知字段；数字参数复用原有解析器。
   实际路由回归覆盖状态码、尝试次数、延迟、账号列表和时间范围，未知字段及非法数字仍拒绝。

## 关键回归

| 路径 | 用例 |
| --- | --- |
| 4xx 草稿保留；不确定失败重新读取 | `runtime-settings.test.mjs` |
| 跨 1000 份、全局保留计数、先扫描后删除 | `retention_scans_all_pages_and_deletes_only_one_batch_per_cycle` 及 Store 分页测试 |
| S3 GET 正文中断后清理 | `connection_probe_attempts_cleanup_after_an_interrupted_get_body` |
| HTTP 中断不终止响应、结算/空闲期中断、非法消息 | `tests/openai/responses/websocket/interrupt.rs` |
| 大目录接受、缓存预算与超大结果不驻留 | transport catalog 与 `client_catalog_byte_budget_evicts_completed_entries_without_rejecting_large_catalogs` |
| 父连接路由、跨 Key/账号、过期不续期、无效 Cookie | `tests/provider/contract/guardian_routing.rs` 及已有 Guardian 调度用例 |
| 指标选择、并发仅负载查询、缓存键及失败恢复 | Admin `usage_metric_*` |
| 相同缓存槽等待总时限 | `usage_metric_budget_includes_waiting_for_the_same_cache_slot` |
| 单项查询与原统计一致，包括恢复基线、精确筛选 | Store `usage_metrics_preserve_selected_values_for_aggregate_and_exact_reads` |
| API 指标、非法参数及完整筛选穿透 | `usage_card_routes_return_only_the_selected_metric_and_validate_options` |
| 卡片独立失败、取消、过期响应；分页和筛选 | `usage-metric-cards.test.mjs`、`usage-key-model.test.mjs`、`usage-filters.test.mjs` |
| 1440/390/320px 布局及交互 | `tests/browser/usage-metric-cards.mjs`、`tests/browser/usage-key-model.mjs` |

## 验证边界

所有构建、测试和浏览器运行在 OVH 独立容器，使用合成上游与隔离数据库；本地只编辑和检查源码。
16 核生产服务器未操作，未使用真实账号请求或修改生产设置。自动回归能验证受控路径的行为，
不能据此声称真实上游零错误、风控下降或首字耗时一定改善。

## 验收结果

| 检查 | 结果 |
| --- | --- |
| OpenAI Provider 单元 / 集成 | 327 / 1148 通过，无忽略 |
| 管理服务 / API / Host / 应用集成 | 400 / 614 / 112 / 62 通过，无忽略 |
| Store | 510 项非 Redis 通过；109 项 Redis 在独立实例重跑全部通过，共覆盖 619 项 |
| PostgreSQL 业务备份恢复 | 真实两轮恢复通过，包含基线与单项统计对照 |
| 前端 Node | 1155 项通过 |
| 前端生产构建 | 最终 Vite build 通过 |
| 浏览器 | 1440 / 390 / 320px 的独立卡片、失败隔离、取消和 Key × 模型分页通过；调度缺失样本补测通过 |
| 静态检查 | workspace 全目标、全特性 Clippy `-D warnings`、cargo fmt、前端全量 ESLint、vue-tsc 通过 |

Store 首次全量中 94 项 Redis 用例报 NOAUTH，应用三个真实集成用例也受影响。
修正方法是新建仅监听隔离网络回环地址 6380 的任务 Redis，不修改业务代码或既有实例认证。
随后全部 Redis 用例及完整应用集成重新执行通过；未将首次失败或环境跳过算成通过。
数据库连接串使用测试夹具要求的 postgres 协议；任务构建容器补齐 pg_dump/psql 后完成真实恢复。

前端全量 ESLint 发现此前 request-location 浏览器脚本一行对象的四项排版问题，
仅拆分属性到独立行后通过，测试行为未改变。
结果日志保存在 OVH `/tmp/cpr-oct30-*.log`；任务源码、产物和截图均未部署至生产。
