# 官方 v3.35.1 原生更新接入记录

## 固定来源和范围

- NewCPR 起点：`2b3a93d0d0a69c0f460456161a73d7962359b363`，包含尚未发布的 Chat 交付修复。
- 上次通用同步点：官方 `80f7d53085f5c9552e7e74b796a0bec50586ee1b`。
- 本轮官方截止点：`12771aca8719ed4b23b0362b23d8d81f228190ca`；正式版为 `d89db84a` / v3.35.1。
- 只接入批准的原生行为，不引入插件运行时、PRISM、请求隐私规则或上游 API-key 账号。
- 官方版本元数据不覆盖 NewCPR 版本。独立账号备注本轮只解释用途，留作单独的数据模型任务。
- 本记录是源码接入验收，不代表已提交、推送、发布或部署。

## 逐项台账

| 来源 | 接入及调用点 | 行为与必要适配 |
| --- | --- | --- |
| `5877b0cb` | OpenAI selector、capacity_wait、turn alias 读取和写入 | 亲和存储预算 100ms 改 500ms；cyber-policy 提示保留独立 100ms。直接选号和排队选号都不得绕过失败的绑定操作发送请求 |
| `6504761c` | API WS Upgrade、Socket、Receiving、connection 活跃通知 | `connection/transport.rs` 与官方文件逐字节一致。沿用本地取消、owner 和租约归属；字节读入立即更新活跃时间，完整消息仍交给原处理器 |
| `958e13c3` | API version rejection / models / WS 升级 | 客户端低于最低版本改为 HTTP 400；两个机器码和版本字段保留，不改后台最低版本或出站 UA |
| `958e13c3` | Core / API / Admin / OpenAI / xAI 的原生错误出口 | 对齐英文模型错误和中文管理提示。下游 Key 参数给出字段级说明；日、周预算分别说明。金额、窗口和结算规则不变 |
| `958e13c3` | Live HTTP/WS 本地传输失败 | 对客户端给安全说明，内部使用已有安全传输诊断。真实上游状态和原始正文仍透传；失败后的 call claim 释放可重试 |
| `958e13c3` | Host update、Admin system error、OperationalDiagnostics | 保留 source 链及脱敏标记；去掉 reqwest 错误中的 URL。检查版本、安装、状态落盘和析构错误都有受控诊断。保留本地安装与已验证镜像部署流程 |
| `958e13c3` | xAI canonical failure | 分类与落库都只从 `error.type` / `response.error.type` 取错误类型，不误用 SSE 顶层事件名 |
| `23b85172` | OpenAI 私有 `compat` 模块、transport 和 provider 调用方 | 纯 body/header 兼容集中管理；通用 Codex normalize 函数按官方复制。账号身份、时区、出站连接仍由原 owner 管理 |
| `23b85172` | API passthrough 和 Provider header filter | 两层都撤销 `x-grok-*` / `x-xai-*` 专属过滤。凭据、账号身份和下游环境头过滤保留；本地本来没有品牌提示词删除逻辑 |
| `23b85172` | Provider HTTP-SSE / WS 托管搜索错误测试 | 补齐 `unsupported_parameter` / `param=tools` 等真实形状。保留原始错误，不删工具、不重试、不换号、不扣账号健康分 |
| `12771aca` | 四处现有帮助入口及 Base 按钮 | 用量明细、版本限制、错误保护和额度预测帮助按钮透明；移除重复原生 title，保留 aria-label、键盘焦点和浮层。已有设置分段导航不重复实现 |
| `b3fa5322` | Protocol SSE delimiter scan / memchr 直接依赖 | LF 和 CRLF 分隔符使用 `memchr::memmem::find`，保留最早分隔符、跨块与尾部语义；没有宣称实测吞吐提升比例 |
| `dca573b7` + `958e13c3` | 上游 WS close 失败投影 | 1009 优先显示非空上游 reason；缺 reason 才用官方通用说明。保留已有 413、禁止重放和不处罚账号 |

## 亲和失败的历史缺口

执行前发现本地不只是预算较短：部分读取、首次 claim 和续期失败仍可能继续选号。
在旧代码上加入 750ms 存储延迟后，测试实际拿到了账号租约，证明存在绕过绑定的路径。
本轮按官方 `binding_store_error` 的效果补齐为 `SessionBound`，让失败在发送前终止，
并禁止 Core 再通过重试绕过。覆盖正常直接选号与排队晋升两条路径。

必要的本地接入差异：本地仍使用已有 binding token/CAS、Redis 等待租约和 root/child
模型，未替换成官方另一套队列实现。独立授权的精确续接保留既有 owner 优先规则；
可选 guardian 父线程偏好及请求完成后的观察写入仍是尽力操作，不冒充发送准入绑定。
CAS 冲突继续受本地快照/排队重选边界约束，不能当作成功续期。
排队中的策略排除读取也限制为 100ms；它保留 NewCPR 已有的读取失败即拒绝合同，
不会因暂时读不到排除名单而放行。普通路径的可丢失策略提示仍按官方尽力读取。

## 首字延迟影响

- 没有新增首字批量缓存、固定等待、等完整响应才交付或主动延后 flush 的策略。
  原生 Responses/Chat 流式交付与现有输出阈值未改；SSE 只替换分隔符查找算法。
- 亲和读取、首次绑定、别名写入及续期发生在发往上游前。单次存储超时从 100ms
  改为 500ms，操作完成立即继续，不是每请求固定等待 500ms。
- 存储抖动时可能更晚发出请求：例如单次读取需要 250ms，现在会等待其完成，旧逻辑
  可能在 100ms 超时后拒绝或错误放行。多个串行存储操作可累计，500ms 不是整条请求上限。
- 绑定竞争时重新读取获胜账号及重选也可能延后发送；这是避免错绑的正确性检查。
  绑定存储失败不再继续发送，因此对应场景可能直接失败，而不是获得一个更慢的首字。
- WS 字节活跃度记录没有新增出站等待条件，但增加少量本地记账；未执行真实流量 TTFT
  对比或性能基准，不能声称所有负载下绝对零开销或保证首字更快。

## 兼容与错误边界

1. `compat` 只操作请求副本。保留已批准的 minimal 别名、无效密文恢复、搜索历史工具索引、
   Excel 和额度透支开关；不从兼容模块改变账号、设备、出口、会话或物理 WS 池。
2. 同步移动 App 架构检查中的两个私有测试 owner，不保留旧路径或扩大为通配豁免。
   WS connection 的源码及测试改为 `connection/mod.rs`，符合现有目录规范；底层
   transport 实现不为通过检查而改写，检查规则也不放宽。
3. `958e13c3` 中插件、隐私和被排除的 Key 查询接口不接入；官方独立 pricing/client-profile
   管理扩展不为文案变更而新增。图像编辑仍使用本地已有的 multipart 解析器及其英文错误，
   不为补句号而整体替换成官方另一套解析器。等价的原生队列和图像权限错误在本地 owner
   对齐文案，算法保持原合同。
   本地创建 Key 尚不接受官方的 `customKey` 等扩展字段，保持原 schema 拒绝，
   不为移植文案而新增功能。测试覆盖本地实际支持的 name、label、限额与并发字段。
4. 原始上游 status/body/code/type/param/message 不被通用安全提示覆盖；安全提示针对本地
   错误。诊断写入失败不覆盖原错误或改变更新终态。
5. 上次复制的 Chat request/response 转换器及最近的收集/交付修复不变。本轮公共 SSE 和错误
   文案会被 Chat 共用，因此仍跑 Chat 与原生 Responses 回归。

## 明确未新增的界面

独立账号备注用于保存管理员说明，与账号名称分开。本地当前没有官方的独立 notes
数据字段，本轮不把自定义名称改成备注，也不只新增一个无法保存的输入框。
官方其他帮助按钮所在的插件、隐私和定价扩展界面不属于本轮范围；本地现有设置导航
已经分段，不为复制布局而改动导航。普通刷新、删除等操作按钮保留原样。

## 关键回归映射

| 行为 | 自动用例 |
| --- | --- |
| 亲和读取、首次绑定、续期的 250ms 成功与 750ms 拒绝；两条准入路径 | `binding_storage_deadlines_cover_direct_and_wait_admission` |
| 并发改变绑定后重新读取获胜账号，释放旧租约 | `binding_renewal_race_reloads_the_winner_on_both_admission_paths` |
| 排队策略读取不跟随亲和预算扩大 | `waiting_policy_storage_keeps_its_independent_100ms_budget` |
| HTTP/WS 搜索错误不重放，完整交付原始错误事件及 tools 参数 | `hosted_search_errors_preserve_wire_details_without_replay_on_http_and_ws` |
| 首次和后续慢上传不中断；停止上传仍会释放连接 | `slow_unfragmented_uploads_survive_heartbeats_on_first_and_later_requests`、`stalled_partial_frame_without_pongs_still_releases_the_connection` |
| 已输出后的 1009 保留具体原因，下一轮重建连接 | `websocket_close_after_delivery_preserves_details_and_reconnects_through_pool` |
| 兼容模块变更不改变账号指纹、路由及缓存身份 | `installation_alias_shape_keeps_account_fingerprint_routing_and_cache` |
| 创建和修改下游 Key 的错误指出字段，但不回显提交值 | `client_key_validation_should_identify_the_invalid_field_without_echoing_values` |

托管搜索按官方检查原始错误和客户端 wire 事件；`param` 不要求在独立的内部错误摘要中
重复存一份。该摘要与客户端原始事件是两个不同边界，不能用摘要字段代替交付断言。

## 验证记录

所有构建、格式化、测试与浏览器验证在用户指定的隔离验证主机执行，未访问生产环境。

| 验证 | 覆盖 | 状态 |
| --- | --- | --- |
| 失败前证据 | 旧 1009 丢 reason；旧 100ms 拒绝 250ms；旧绑定失败仍拿到租约 | 已复现 |
| Rust 工作区 | API、Core、Admin、Host、Protocol、Store、两个 Provider 和 App；显式启用隔离 PostgreSQL/Redis，CI 模式拒绝缺环境跳过 | 已完成全范围运行；首轮失败的 Store / OpenAI 经修正后分别整包复验通过，详见下文 |
| 新增协议回归 | 慢上传与停传、亲和 250/750ms、HTTP/WS 托管搜索、1009 新旧连接、Live 代理失败、系统诊断脱敏 | 已通过；另显式执行 2 项默认忽略的回环协议测试，身份/路由/缓存用例连续复测 5 次通过 |
| Rust 质量检查 | fmt、Clippy、架构 owner / 依赖边界 | 最新源码 fmt、工作区 all-target Clippy（warnings 视为错误）和 App 架构检查通过 |
| 前端 | 类型、ESLint、生产构建、测试 | 已通过；1,139 项测试通过、无失败或跳过 |
| 浏览器 | 实际 Vue 组件；1440px / 390px；透明 normal/hover/active、去重复 title、焦点、帮助浮层、无横向溢出 | 已通过；主题初始化后重新验收 |
| 源码一致性 | 本地待交付 backend/frontend 与隔离主机已验证源码的逐文件 checksum | 一致；没有仅在验证主机保留的产品修复 |

最终分包结果：App 62；Admin 9 单元 + 395 集成；API 2 + 607；Core 8 + 515；
Host 23 + 112；Protocol 90；Store 12 + 613；OpenAI 327 + 1,142，另显式运行
2 项回环协议测试；xAI 390。Doc tests 通过。Host 的 1 项真实外部内核下载测试仍未执行。

验收过程保留失败事实：完整工作区命令首次有 Store 2 项测试环境错误和 OpenAI 2 项新增
夹具错误，不能记作该命令一次全绿。Store 测试 URL 补齐备份/恢复用例要求的密码字段，
不改产品或数据库认证；OpenAI 夹具启用真实模拟租约，并容许 Tokio 毫秒时钟取整。
之后两个包均完整复验通过，最后重新执行严格 Clippy 和格式检查。
更早的一轮曾有 1 次既有指纹 HTTP 捕获测试失败；没有修改该产品路径或用例，后续工作区、
OpenAI 整包及连续 5 次专项复测通过，尚未确定该单次失败的根因，不能宣称已修复其根因。

未做生产流量抓包、真实上游账号压测或 SSE 性能基准。模拟上游和隔离数据库通过不代表
真实上游一定接受某个工具，也不证明风控或生产错误率下降。提交状态以 Git 记录为准；
本轮验收未推送或部署。
