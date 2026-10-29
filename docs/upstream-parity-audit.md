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
| 严格子线程等待根账号恢复或迁移 | 官方 `74f749f4`，2026-10-06；本地 `d79d8086ba`，2026-10-07，纳入 v3.21.0 | 只保留并发/间隔等待。根账号失效或被本次请求排除时，子线程很快无可用账号失败；未完整保留 follow-only 的终止语义 | 本次修复 |
| 已知模型的 Provider 归属 | 官方 `2f96c02e`，已包含在 v3.25.0 的官方基线 `d700df8a` | 声明支持模型的 Provider 优先，但未声明支持它的发现型 Provider 仍可作为候选；可能制造无效跨 Provider 请求 | 已确认历史遗漏，尚未修改，建议单独补齐 |
| 目录获取失败保留最后已知模型事实 | 同为官方 `2f96c02e`，v3.25.0 集成范围 | 单 Provider 目录读取报错时被跳过，编译仍可成功发布缺少旧模型事实的快照，模型列表及路由判断可能退化 | 已确认历史遗漏，尚未修改，建议单独补齐 |

上述第二、第三项由历史源码控制流确认，本次没有用生产流量复现其发生频率。
不能把它们描述成已验证导致当前生产错误，也不能描述成已经修复。

### 本次故障的核验依据

2026-10-29 11:14:40 至 11:14:52（Asia/Shanghai）的只读日志及详细采集记录中，
共确认六次上游 `401 token_revoked`：三个普通请求切到另一账号后返回 200，
三个 `thread_spawn` 子线程仍跟随失效根账号并返回 503。另一账号当时为 3 至 5 / 30
并发，失败不是由账号池满载或换号次数耗尽造成；原账号于 11:14:55 重登成功。
对应失败尝试尚未交付下游、可安全重放，也没有原生续接 owner 屏障。
真实账号标识、请求正文和凭据不纳入此报告。

### 本次修复的效果和边界

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

### 其他两项的最小补法

- **Provider 归属**：在现有 snapshot planner 中先确定映射后模型的声明者。
  已知模型只路由声明者，未知模型保留唯一合格 Provider 透传；多个发现型 Provider
  无法确定归属时按官方拒绝歧义。补原生 OAuth 的 Grok 资格防线，单独核对 Excel 边界。
  保留现有账号评分、身份、出口和 scope，不引入插件或上游 API-key 账号。
- **目录失败回退**：仅在单个 Provider 读取目录失败时复用旧模型、能力、展示和目录账号事实。
  当前账号、权限、禁用和容量必须来自本次新配置；不能回退整个旧快照。
  成功返回空目录必须真正撤回模型，不能用旧缓存掩盖。
- 必须新增的回归：强制错误 Provider、声明者不可用/越权、共享声明模型、模型映射、
  未知单/多 Provider、目录失败同时撤销账号、成功空目录、冷启动和目录 generation 变化。

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
  `tests/provider/contract/model_account_eligibility.rs`。本地
  `backend/crates/gateway-core/src/routing/snapshot.rs::plan` 仍追加未声明模型的候选；
  `tests/routing/snapshot.rs::known_model_precedes_discovery_fallback_without_losing_passthrough`
  明确固定旧的 fallback 行为。v3.24.0 到 v3.25.0 对应实现没有更新。
- F2：官方同提交的 `catalog_failure_should_preserve_model_facts_without_restoring_revoked_accounts`。
  本地 Core `routing/snapshot.rs::compile_runtime_snapshot` 的目录 Err 分支直接跳过；
  `runtime/mod.rs` 的后台刷新使用实时 compile，已有 cached 模式只服务配置发布，不能替代失败回退。
- 原生恢复：OpenAI `tests/provider/contract/metadata_recovery.rs`、`precommit.rs`、
  `tls_recovery.rs`、`native_encrypted_history.rs`；Core `tests/engine/coordinator.rs`。
- 身份和 WS：OpenAI `tests/provider/contract/identity_isolation.rs`、
  `src/transport/websocket/pool/lifecycle_tests.rs`；API
  `tests/openai/responses/websocket/connection.rs`。
- 预算和保护：Store `tests/postgres/client_budget_buffer.rs`；Core
  `tests/engine/account_protection.rs`；Store `tests/redis/credential_cooldown.rs`。

## 不应误判为本次漏合并的差异

- Chat 非流式转换保留已有 CPA 兼容：custom/image/reasoning、原始参数和部分不完整工具项。
  `backend/crates/gateway-protocol/tests/openai/chat/cpa_response.rs` 有明确断言。
  官方对畸形身份更严格，本地并非完全等价；本次没有证明该差异造成现实故障，需独立评估，
  不直接用官方转换层覆盖现有协议能力。
- 账号关联隔离的 turn/date/Live 修正出现在官方 2026-10-23，晚于本地 v3.29.0；
  NewCPR `60ec27b98` 已在 v3.30.0 跟进。旧 `client-continuity.md` 的 turn 映射说明
  已滞后，本次修正文档，运行时代码本身不回退。
- 进程内物理 WS 容量、Redis 业务租约、智能评分及本地预算窗口属于有记录的架构适配。
  是否等价必须检查实际入口与负面测试，不能按模块名称不同判断为弱化。

## 验证记录

所有可执行验证在隔离测试服务器完成，使用合成账号和回环上游；不使用生产账户或生产数据库。
验证对象为审计基线上的本次未提交修改。本次不发布版本、不部署、不修改线上配置。

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
