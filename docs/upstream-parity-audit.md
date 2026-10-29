# 官方功能适配回查：v3.21.0 至 v3.32.0

审计日期：2026-10-29。范围是最近十五个 NewCPR 发行版本的重要原生请求行为，
重点包括账号调度、失败恢复、模型路由、HTTP/WS、身份隔离、续接和结算。
这不是全库、全部 UI 或真实上游风控的无缺陷保证。

## 固定基线

- 开发目标：`Heykode/newcpr`。
- 审计前源码：`e4b90051b33e8d5038dfeb1e87e49b868140acfd`，即 v3.32.0。
- 最近已适配官方截止：`80f7d53085f5c9552e7e74b796a0bec50586ee1b`。
- 本次核对官方源码：`958e13c370d05e005daa931ba9b98cd5a9c2c312`。
  截止之后的新提交不归类为过去漏合并。
- 排除已经取消或不需要的 PRISM、插件运行时、上游 API-key 账号和 Codex 隐私规则。
  混合提交中适用于原生账号池的部分仍单独核对，不能随插件部分一起丢弃。

结论依据是历史标签对象、当时采用的官方提交、实际实现及测试断言，
不以提交标题、文件相似度或当前测试通过替代功能等价性判断。

## 确认的缺口

| 项目 | 历史来源 | 修复前的行为与影响 | 处理状态 |
| --- | --- | --- | --- |
| 严格子线程等待根账号恢复或迁移 | 官方 `74f749f4`，2026-10-06；本地 `d79d8086ba`，2026-10-07，纳入 v3.21.0 | 只保留并发/间隔等待。根账号失效或被本次请求排除时，子线程很快无可用账号失败；未完整保留 follow-only 的终止语义 | 已提交 `f68ac36f3` |
| 已知模型的 Provider 归属 | 官方 `2f96c02e`，已包含在 v3.25.0 的官方基线 `d700df8a` | 声明支持模型的 Provider 优先，但未声明支持它的发现型 Provider 仍可作为候选；可能制造无效跨 Provider 请求 | 已提交 `8a31e2b66`，验证见下 |
| 目录获取失败保留最后已知模型事实 | 同为官方 `2f96c02e`，v3.25.0 集成范围 | 单 Provider 目录读取报错时被跳过，编译仍可成功发布缺少旧模型事实的快照，模型列表及路由判断可能退化 | 已提交 `8a31e2b66`，验证见下 |
| 原子失败事件中的观测兜底 | 官方 `b8479b59` 中可独立用于原生的 Core 逻辑 | 仅在失败批次附带的请求 ID/状态可能在换号前漏记；已用合成测试复现 | 本轮按官方循环补齐，非生产故障归因 |
| Chat 非流式工具身份 | 官方 `005466bc`，已有 CPA 兼容策略差异 | 无效工具 ID/名称仍返回成功，下一轮入口又拒绝；已用 Protocol/API 测试复现 | 本轮适配有效身份规则，保留正常扩展能力 |

上述第二、第三项由历史源码控制流确认，本次没有用生产流量复现其发生频率。
不能把它们描述成已验证导致当前生产错误；本轮代码补齐也不代表已发布到生产。

### 本次故障的核验依据

2026-10-29 11:14:40 至 11:14:52（Asia/Shanghai）的只读日志及详细采集记录中，
共确认六次上游 `401 token_revoked`：三个普通请求切到另一账号后返回 200，
三个 `thread_spawn` 子线程仍跟随失效根账号并返回 503。另一账号当时为 3 至 5 / 30
并发，失败不是由账号池满载或换号次数耗尽造成；原账号于 11:14:55 重登成功。
对应失败尝试尚未交付下游、可安全重放，也没有原生续接 owner 屏障。
真实账号标识、请求正文和凭据不纳入此报告。

### 严格子线程修复的效果和边界

1. strict 子线程在根账号暂时失效、冷却、停用、排除或不可选时，进入有界等待。
   普通 Busy 等待开关关闭时仍保留该行为，复用现有 sticky 人数与超时设置。
2. 根线程实际迁至 B 后，子线程释放旧等待租约，重新读取完整冻结候选集合，跟随 B。
   不再只从已经按 A 筛过的候选列表里找 B。绑定消失时沿既有 CAS 认领合法账号。
3. 根迁移不重开等待窗口；排队中观察到的迁移不消耗快照冲突次数。
   绑定在获取执行槽期间变化时释放旧槽，重新核对 owner。
4. 队满、超时和 follow-only 选择失败禁止借其他 Provider 逃离根绑定。
   明确 required account、Native/ReplayOwner 和管理员诊断保留各自原有恢复合同。
5. 重新登录不会自动清除同一请求已经记录的失败账号排除。
   已因该请求的 401 排除 A 时，仍需要等待合法根迁移等条件；不保证重登后旧请求必定成功。
6. 等待不占执行并发，不发送模型请求，不改变 UA、设备、出口、账单或加密历史。
   账号权限、模型权限、额度、冷却及执行租约都要在发送前重新核对。
7. 初始已绑定且有权限的停用根账号可纳入首次冻结集合，以支持重新启用恢复；
   等待期间新导入的账号不能扩入当前集合。此为 NewCPR 已有冻结权限/候选合同的适配边界。

官方同样存在有界等待超时，并不是无限等待或任意跨账号恢复。
本修复提高暂时不可用时的恢复机会，不证明上游风控降低，也不保证每个 401 都能无感恢复。

### 为什么之前漏掉

历史 `d79d8086ba` 迁移保留了 root pin，却没有完整迁入官方的硬不可用等待和
`SessionBound` 错误包装。旧的容量等待规范把“失效、冷却等不得等待”写成通用约束，
没有区分普通可选等待与必须跟随根账号的子线程。原测试覆盖正常亲和、并发忙和间隔，
没有覆盖这次失效根账号的恢复分支。代码和测试合同确实不完整；无法从 Git 推断作者动机。

本次同步修正容量等待、历史迁移和官方集成规范，要求以成功、阻塞、恢复、超时行为矩阵
核对上游；不能用“保留本地架构”作为省略已批准上游行为的理由。

### 本轮补齐的另外两项

- **Provider 归属**：按官方 `2f96c02e` 在现有 snapshot planner 中先确定映射后模型的声明者。
  已知模型只路由声明者，未知模型保留唯一合格 Provider 透传；多个发现型 Provider
  无法确定归属时按官方拒绝歧义。即时/等待选择器共同阻止原生 Codex OAuth 接收
  `grok` / `grok-*`，含大小写与路径前缀变体；按模型解析为 Excel 的路径保留原资格合同。
  同时保留合法未知原生型号透传、管理员固定账号诊断和质量检测原有的模型政策豁免。
  保留现有账号评分、身份、出口和 scope，不引入插件或上游 API-key 账号。
- **目录失败回退**：仅在单个 Provider 读取目录失败时复用旧模型、能力、展示和目录账号事实。
  当前账号、权限、禁用和容量必须来自本次新配置；不能回退整个旧快照。
  成功返回空目录必须真正撤回模型，不能用旧缓存掩盖。
  现有配置提交仍只复用缓存、不访问远端；后台刷新保留 generation 稳定性校验，
  以及配置优先发布和取消语义。其他 Provider 的成功更新可正常发布。
- 新增回归覆盖强制错误 Provider、声明者不可用/越权、共享声明模型、模型映射、
  未知单/多 Provider、目录失败同时撤销账号、成功空目录、冷启动、目录 generation
  变化、HTTP/WS 选择器资格，以及管理员诊断和 Excel 准备。保留既有配置抢占回归。

## 十五版逐项结果

“未发现新增遗漏”仅指本行列出的重点行为，不代表该版本全部功能无缺陷。
F0、F1、F2 分别指上表的严格等待、Provider 归属、目录失败回退。

| NewCPR 版本 | 标签提交 | 重点回查内容 | 结果 |
| --- | --- | --- | --- |
| v3.21.0 | `22e578ee2` | strict/relaxed、Guardian、刷新错峰、Retry-After、Live owner、绑定 CAS | F0 从此版存在；其他所查合同有等价实现 |
| v3.22.0 | `962a49d76` | 点数展示、预热出口、自动重置卡、reasoning 参数 | 所查合同保留；这些含本地增强，不虚构统一官方 pin |
| v3.22.1 | `a52b06b0f` | 日志清理热修及请求链路差异 | 没有新的原生调度/恢复改动；沿用已有 F0 |
| v3.23.0 | `ae1f33e65` | Preferred、首次原子认领、续接选号前校验、Guardian 独立并发 | 所查新增合同保留；F0 仍在 |
| v3.23.1 | `76db3221b` | minimal 兼容、无效加密历史同账号恢复 | 本地增强保留，发送及交付边界未放宽 |
| v3.24.0 | `fda63bc79` | 已知模型优先、生图资格、WS 中断、关闭连接与输出前恢复 | 符合当时官方 `eea16f52` 的所查合同；F1/F2 尚未出现在该 pin |
| v3.25.0 | `9d1b80c72` | 显式重放证明、503、轮次限流、额度观测竞态、cookie、SSE 保活 | 确认遗漏 F1/F2；其他所查恢复合同保留 |
| v3.26.0 | `48556a2b2` | metadata 后恢复、失败轮次标记、reset 时间 | 所查新增合同保留；F0/F1/F2 延续 |
| v3.27.0 | `99ffc4d1c` | 前导即时交付、Search 权限、物理 WS 容量 | 前导交付延迟已由 `1cce8f9d2` 修正；其他所查合同保留 |
| v3.28.0 | `3e1b7d191` | credits 恢复、搜索历史补工具、首响应指标 | 所查合同保留；本地统计架构不同，不按缺文件判遗漏 |
| v3.29.0 | `225b1b7a6` | opening breaker、建连轮换预算、daemon 恢复、账号关联隔离 | 所查合同保留；后来的 turn/date 修正不倒算为本版遗漏 |
| v3.30.0 | `7bcbc7d42` | WS 回收、异步预算、诊断、流内404、turn/date/Live 修正 | 所查合同保留；有后续官方修正的实际适配证据 |
| v3.30.1 | `9e8da61b3` | 本地透支探测修复 | 无新增官方批次，不虚构漏移植清单 |
| v3.31.0 | `215a92cb3` | WS→SSE Retry-After、连接画像、模型观测字段删除 | 所查合同保留；模型计费仍采用实际发送模型 |
| v3.32.0 | `e4b90051b` | Chat、双向WS生命周期、错误保护、模型目录、Live资格、续接effort | 所查新增合同保留；Chat buffered 有明确兼容差异，见下文 |

## 关键证据入口

- F0：官方 `74f749f4` 的 OpenAI `credential/selector.rs`，以及
  `tests/provider/contract/session_binding.rs`。本地历史 `d79d8086ba`；本次入口为
  `backend/crates/providers/openai/src/credential/selector/capacity_wait.rs`、
  `selector/mod.rs`、`provider/observation.rs` 和
  `backend/crates/gateway-core/src/engine/capacity_wait.rs`。
- F0 回归：`backend/crates/providers/openai/tests/provider/contract/affinity_recovery.rs`，
  覆盖等待开/关、凭据失效/停用恢复、实际根迁移后完成 SSE、失败排除、超时/队满、
  多次迁移同一截止、冻结候选、获取执行槽竞态、绑定消失，以及真实回环401/429。
- F1：官方 `2f96c02e` 的 Core `routing/snapshot.rs` 与 OpenAI
  `tests/provider/contract/model_account_eligibility.rs`。审计基线的
  `backend/crates/gateway-core/src/routing/snapshot.rs::plan` 仍追加未声明模型的候选；
  原 `known_model_precedes_discovery_fallback_without_losing_passthrough` 测试固定旧行为。
  v3.24.0 到 v3.25.0 对应实现没有更新。本轮已替换为归属/歧义合同，
  并新增 `known_model_ownership_survives_mapping_and_owner_restrictions`、
  `unknown_models_require_one_eligible_discovery_provider` 和 OpenAI 模型资格回归。
- F2：官方同提交的 `catalog_failure_should_preserve_model_facts_without_restoring_revoked_accounts`。
  审计基线 Core `routing/snapshot.rs::compile_runtime_snapshot` 的目录 Err 分支直接跳过；
  已有 cached 模式只服务配置发布，不能替代失败回退。本轮在 `runtime/mod.rs` 后台入口
  使用 `compile_with_catalog_fallback`；`tests/runtime/catalog_fallback.rs` 覆盖模型事实
  保留、当前账号/权限/容量、独立更新、成功空目录、冷启动、缓存提交与 generation 变化。
- 原生恢复：OpenAI `tests/provider/contract/metadata_recovery.rs`、`precommit.rs`、
  `tls_recovery.rs`、`native_encrypted_history.rs`；Core `tests/engine/coordinator.rs`。
- 身份和 WS：OpenAI `tests/provider/contract/identity_isolation.rs`、
  `src/transport/websocket/pool/lifecycle_tests.rs`；API
  `tests/openai/responses/websocket/connection.rs`。
- 预算和保护：Store `tests/postgres/client_budget_buffer.rs`；Core
  `tests/engine/account_protection.rs`；Store `tests/redis/credential_cooldown.rs`。

## 不应误判为本次漏合并的差异

- Chat 非流式转换保留已有 CPA 兼容：custom/image/reasoning、原始参数及可选内部 item ID。
  初次审计只记录工具身份校验差异；本轮独立验证已复现无效回复会阻断下一轮续接，
  因而补入非空工具身份和唯一 call ID 校验。没有用官方转换层覆盖现有扩展能力，
  具体差异、影响和风控证据边界见下文。
- 账号关联隔离的 turn/date/Live 修正出现在官方 2026-10-23，晚于本地 v3.29.0；
  NewCPR `60ec27b98` 已在 v3.30.0 跟进。旧 `client-continuity.md` 的 turn 映射说明
  已滞后，本次修正文档，运行时代码本身不回退。
- 进程内物理 WS 容量、Redis 业务租约、智能评分及本地预算窗口属于有记录的架构适配。
  是否等价必须检查实际入口与负面测试，不能按模块名称不同判断为弱化。

## 提交后补查覆盖范围

补查基线为 `8a31e2b66`，官方范围仍截止 `80f7d530`。本轮重新检查官方历史提交的
实际 diff 与本地入口，重点补查此前十五版表格没有单列的合同；没有重新执行历史
二进制、浏览器验收、数据库故障注入或真实上游流量。本节源码检查不冒充新增测试通过。

| 官方来源 | 本轮补查内容 | 本地结果与证据入口 |
| --- | --- | --- |
| `be5b6d19` | 官方客户端识别、版本门禁、SemVer build 元数据 | 已有：API `openai/auth.rs::identify_codex_client` 包含官方产品名，Core `policy/client_version.rs` 使用 `cmp_precedence`；已有未知产品兼容与真实 HTTP/WS 门禁测试 |
| `286031a4` | 管理员访问与刷新凭据分离、并发刷新与重放 | 已有：Admin `use_case/auth/session.rs` 区分 `a2`/`r2`、CAS 轮换和有界旧凭据重放；API `admin/auth.rs` 使用两个 cookie，前端 auth store 合并并发刷新；测试 `refresh_credentials_are_separated_and_replay_outside_grace_revokes_session` 覆盖主要边界 |
| `3ef4743e` | 前导/心跳已经交付后，仍允许安全重试落库 | 已有：Store `postgres/execution.rs::begin_model_request_attempt` 只要求运行态和连续序号，保留交付事实和发送水位；Core 决定能否重放，不由存储层一概拒绝 |
| `a82bcdb8` | 选号耗尽后保留真实上游失败、区分会话策略排除 | 已有：Core `engine/coordinator.rs` 保留 `last_retryable_failure`，OpenAI `provider/observation.rs` 独立映射 `SessionPolicyRestricted`；本地另有明确的账号认证 401 不能冒充下游鉴权错误处理 |
| `b2dab66a` | 单条坏导入记录不能拖累其他账号 | 已有：前端 `account-import-batches.ts::splitAccountDocuments` 拆分 OpenAI 条目，`useAccountOnboarding.ts` 按独立条目创建持久导入任务；保留 xAI 去重、模板、2FA 和批次幂等，本轮未重新做浏览器导入验收 |
| `f7f59103` 的账号分组部分 | 保存分组成功后，读取展示资料失败不能把保存报告为失败 | 已有：Admin `account_groups.rs` 先发布已提交配置，展示失败清空可选 record；Store `account_groups/mod.rs::committed_record` 不回滚已完成的变更 |
| `b8479b59` 的共享性能部分 | 共享不可变响应元数据和响应头，避免每个事件深拷贝 | 已有：Core `event.rs` 使用 `Arc<str>` 和 `Arc<[ProviderResponseHeader]>`；同提交的错误附带观测兜底另见下文，不能因标题是插件就整项排除 |
| `005466bc` | Chat 非流式工具身份校验 | 后续独立验证已复现并补齐：拒绝缺失、空白、非字符串身份及重复 call ID；保留 custom/image/reasoning、原始参数和可选内部 item ID |

### 共享层失败观测兜底，本轮补齐

官方 `b8479b59` 在 Core `begin_stream_failure` 取走当前 attempt 之前，遍历
`atomic_client_events` 并读取其中的 `response_observation`。相应官方用例
`atomic_response_failed_should_rotate_account_before_stream_commit` 给失败事件附上
请求 ID，要求换号后的中间失败记录仍保存该 ID。

修复前本地对应路径在可重试分支只读取 `canonical_facts`，终止交付时才由
`observe_atomic_terminal_events` 读取附带观测。因而若一个 Provider 只通过失败事件
附带观测，换号前的中间记录可能缺少该观测中的请求 ID 或状态。共享层这段兜底确实
没有移入，上一轮补查才单独列出。

影响必须区分：当前 OpenAI 原生和 Excel 复用的 `provider/execution.rs` 会先独立
yield `ProviderEvent::observation`，然后才返回失败；xAI 同样独立发送观测，且没有
该种原子失败事件生产路径。本轮全源码搜索没有发现原生 Provider 把观测
仅放在 `with_atomic_client_events` 的失败批次内。因此这不是已经证实的当前 Codex
丢诊断或重试故障，也不能据此解释生产错误。

本轮先在隔离环境复现官方用例：失败记录请求 ID 实际为 `None`，预期有值。
随后按官方的观测循环补齐，在取走当前 attempt 及恢复旧批次之前读取诊断。
不提前消费计量、原始事件或改变重试资格。显式错误 ID/状态优先；没有显式值才读取
观测；新账号不继承旧 ID、状态和耗时。选号耗尽时诊断留在原失败 attempt 记录，
不凭空给不存在的新 attempt 归属身份；恢复旧批次仍只累计一次费用。

### Chat 工具校验专项及账号风险核验

固定来源为官方 `005466bcb0f5154f36f495cd8cd34ac267ceaf31`（PR #542）。
官方提交的生产改动位于 API 协议转换和交付，未修改 Core、Provider、Store。
本地 `gateway-api/src/openai/chat/http.rs` 先解码请求，再调用原 Generate 服务；
`responses/http.rs::collect_execution_response_as` 先收集上游，再转换 Chat，最后提交
下游成功。因而本次非流式输出校验不能改变该次已发往上游的请求或账号身份。

| 场景 | 修复前 | 本轮行为与依据 |
| --- | --- | --- |
| 工具缺失、空白或非字符串 call ID/名称 | 可返回 200 和不可用工具项；下次 Chat 入口拒绝 | 参考官方身份校验并对齐本地入口，返回明确 502，在成功提交前走现有失败收尾 |
| 两个工具使用同一 call ID | 可返回歧义结果；下一轮无法匹配 | 同一回复内禁止重复，function/custom 共用唯一性检查 |
| 正常 function/custom、图片、推理内容 | 支持 | 保留；原始 arguments/input 不修补、不要求能解析成 JSON |
| 重复或缺失内部 item ID | 非流式转换不依赖该 ID | 保留；它不作为 Chat tool call ID 交给客户端，不照搬无关限制 |
| 原生 Responses、Chat 流式及账号选路 | 原有链路 | 不增加协议规则；共享 Core 只增加失败观测读取 |

红测分别复现 Protocol 返回空工具 ID、API 返回 200。往返测试同时保留有效 custom
输入、原始未闭合参数、图片和推理内容；API 测试验证已有非法输入在启动账号执行前
即被拒绝。官方输出仅检查字符串是否为空；本地额外拒绝纯空白，以匹配既有入口的
`nonempty` 规则，并非声称逐行一致。本次输出校验不新增上游重试，但下游客户端
可能对 502 自行重试。交付失败沿用 Core 本地失败路径，不额外记录 Provider 健康失败。

结论：这是工具调用完整性修复，不能据此宣称更少封号、401 或 429。没有证据证明
官方改动的目的是降低账号风控，也未做真实账号风险实验。不是无条件更严格就更好。

### 仍未穷尽的范围

- 已完成最近十五版所列重要原生请求行为的静态回查，并补查上述八组合同；并非全部
  官方提交的每一行都做了等价证明。尤其不能把有意保留的兼容差异称为逐行 1:1。
- 全部 UI、构建发布脚本、备份恢复和多实例高负载尚未全量验收；真实 Live 音视频、
  credits 消费、全部客户端和真实上游风控未实测。不存在测试结果的部分不声明通过。
- 共享层失败观测兜底及无效工具身份在本轮补齐；Chat 有意保留的扩展和容错已单独列明。
  补查的上述合同未新增确认当前原生路径的其他功能遗漏；这不是整个项目不存在其他问题的保证。

## 验证记录

所有可执行验证在隔离测试服务器完成，使用合成账号和回环上游；不使用生产账户或生产数据库。
本次不发布版本、不部署、不修改线上配置。以下区分初次严格恢复修复与本轮模型补齐。

### 初次严格恢复修复（已提交 `f68ac36f3`）

- `cargo test -p provider-openai -p gateway-core --locked -- --test-threads=1`：
  最终通过 **1975 项，0 失败，2 忽略**。Core 为 515 项，OpenAI 为 1460 项；
  新增的十个严格恢复场景全部通过，包括普通等待开/关时的已确认额度耗尽。
- 忽略项是显式手动采样 `capture_zstd_upgrade_corpus` 和 `capture_tls_upgrade_matrix`，
  本次未执行，不计入通过数；普通压缩及 TLS 回归仍包含在完整测试中。
- `cargo clippy --workspace --all-targets --locked -- -D warnings`：最终通过。
- `cargo fmt --all --check`：最终通过。
- `cargo test -p codex-proxy-rs --locked architecture -- --test-threads=1`：14 项通过。
- 隔离 Redis 的 `redis::capacity_wait`：29 项通过；
  `redis::provider_session_affinity`：13 项通过。实际连接测试 Redis，不以环境缺失跳过冒充通过。
- 最终重编译前移除临时诊断输出，刷新所有修改 Rust 文件的构建时间；
  本地与远端九个 Rust 文件 SHA-256 一致。一次早期额度用例结果与重新编译结果不一致，
  因此最终重新运行完整 Core/OpenAI 测试，不以早期增量构建结果验收。
- 最后阶段 SSH 断连，重连后确认远端测试已全部完成，重新执行格式检查成功；
  没有将断连或仍在执行的任务当作完成。
- 本地仅检查源码、Git 差异及文件哈希，`git diff --check` 通过；没有在本机运行构建或测试。
- 历史十五版为静态审计；本次测试不等于逐个历史二进制重跑，不证明真实上游风控或所有客户端兼容性。

### 本轮模型归属和目录失败补齐

- Core 全套通过 521 项（8 单元 + 513 集成），包含新增的六个场景和原有严格恢复、
  管理诊断、质量检测、权限及配置抢占回归。
- OpenAI 首次完整运行：327 单元通过；集成 1135 通过、1 失败、2 忽略。
  九个新增场景中的三个 Provider 场景全部通过。
- 唯一失败为未修改的 `compare_legacy_and_upstream_connection_defaults`：
  专用 `native-h1-legacy` 对照组超出 20 秒；同次 `native-h1-current` 正常。
  不修改代码或放宽断言，连续三次完整定向复测均通过（各约 2 秒）。
  该偶发项不能描述成首轮全套通过，也不能据此证明真实网络没有时序问题。
- 两项忽略仍为手动 TLS / zstd 采样，本轮未执行。
- API 全套 599 项（2 单元 + 597 集成）通过，含模型列表权限/失败、HTTP/WS 和管理员诊断接口。
- Store `postgres::snapshot::` 四项通过，使用真实隔离 PostgreSQL 且启用 CI 缺环境即失败约束，
  不把缺数据库导致的提前返回算作通过。App 架构 14 项通过。
- `cargo clippy --workspace --all-targets --locked -- -D warnings` 与
  `cargo fmt --all --check` 最终通过。远端验证进程退出 0，未遗留构建或测试任务。
- 本地与远端十个 Rust 文件 SHA-256 一致，`git diff --check` 通过。
- Excel 新增用例验证选择与准备，未发送到真实 Excel 服务；本轮也未进行真实上游风控实验。

### 失败观测和 Chat 校验后续验证

- 固定基线 `f18500b94`；先验证失败再修改实现。Core 官方场景复现中间请求 ID 丢失，
  Protocol 复现空工具 ID 被接受，API 复现无效工具回复返回 HTTP 200。
- 修复后 Core 原子失败专项 12 项、Chat 协议 147 项通过。API 新增输出失败用例验证
  HTTP 502、单次错误终结、取消及无成功内容；无效续接输入在执行账号前拒绝。
- API 完整运行：2 单元通过，集成 598 通过、1 失败。唯一失败是上传过程带入 AppleDouble
  附属文件，导致冻结文件清单不匹配。确认文件类型并只删除这八个传输残留后，API
  架构 5 项复测通过；没有修改清单检查、业务实现或已有断言来放宽检查。
- 中断前未取回的完整检查退出结果不计为通过；上述结果来自保存了输出的新一轮运行。
- Core 全套 523 项（8 单元 + 515 集成）、Protocol 全套 239 项通过。
  OpenAI 327 单元 + 1136 集成通过；原有两项手动 TLS/zstd 采样忽略，未运行。
  本轮包含账号身份/出口、HTTP/WS、续接、失败回放和账单收尾的现有回归，
  不把回环测试解释为真实上游风控实验。
- 应用架构 14 项、`cargo clippy --workspace --all-targets --locked -- -D warnings`、
  `cargo fmt --all --check` 通过，最终远端验证退出码为 0。后端受版本控制的全部文件
  SHA-256 与隔离测试源码一致，包含新增测试文件。验证进程已结束。
- 所有构建、测试、格式检查均在隔离测试服务器运行；本机仅源码/Git/哈希及隐私检查。
  本轮没有推送、合并、打标签、发布、部署或修改生产配置。
