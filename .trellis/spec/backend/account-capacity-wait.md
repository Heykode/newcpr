# 账号容量等待合同

## 1. 范围与证据边界

适用于 OpenAI 正常请求在账号执行并发不足或请求间隔未到时的可选等待，以及 strict 子线程的
有界根账号跟随等待。后者不受普通 Busy 等待开关控制。等待不是 WS socket
排队，不是新的调度算法，也不增加执行并发。管理诊断走原 selector 路径；
不能据此要求 OAuth 刷新、后台额度/目录任务或其他 Provider 自动排队。

本文维护当前 Core、OpenAI Provider 和 Redis Store 的运行合同。
本次官方基线、接入差异及验证边界见 `docs/upstream-v3421-native-parity.md`；
历史任务或曾经通过的测试不替代当前行为核验。后续更新遵循
`docs/upstream-integration-checklist.md`，同提交淘汰冲突的旧条款和旧预期。

## 2. 单一权威与入口

| 权威 | 实际入口与职责 |
| --- | --- |
| Core 调度 | `account/selection.rs::AccountSelector` 统一排序及 `Ready` / `Busy` / `Blocked` 分类，保留 Smart、权重方向和亲和语义 |
| Core 请求生命周期 | `engine/capacity_wait.rs::AccountWaitBudget` 保存共享截止；`engine/coordinator.rs` 向各 attempt 传同一句柄并控制恢复 |
| Core 端口 | `provider_ports/capacity_wait.rs::ProviderWaitLease` 定义等待、晋升和释放，不解释 Redis 数据 |
| OpenAI Provider | `credential/selector/mod.rs` 控制开关；`credential/selector/capacity_wait.rs` 编排候选、等待和凭据终检 |
| Store | `redis/capacity_wait.rs` 原子准入、晋升、取消和清理；`bundle.rs` / `workers.rs` 注册生命周期 worker |
| 配置及容量 | 既有 `RequestTuning` JSONB 是等待配置来源；已提交运行快照发布的 `AccountConcurrencyHandle` 是 live 容量来源 |

表中源码路径相对于各自 crate 的 `src/`。API 不建立队列，Provider 不直接操作
Redis key；WS pool 不兼任账号调度器。不新建配置表、第二套粘性状态或容量缓存。

## 3. 配置合同

沿既有 runtime settings API、Admin `RequestTuningOverrides`、
`runtime_settings.request_tuning_json`、snapshot 映射和请求计划消费以下字段：

| JSON 字段 | 默认值 | 校验 |
| --- | --- | --- |
| `accountBusyWaitEnabled` | `false` | boolean |
| `maxWaitingPerAccount` | `10` | 整数，`1..=1000` |
| `accountQueueTimeoutSeconds` | `30` | 整数，`1..=120` |

2026-11-01 用户批准统一为两项：sticky、fallback 和 strict follow-only 等待读取
同一份等待配置；普通池按人数上限，Reserved 按下文 High 优先级免普通单账号上限。
时限跨模式、跨 attempt 不得重开共享截止。
迁移0083将旧四项统一到10/30，保留已经保存的新两项及其他所有设置。
Web只展示两个账号等待输入，Key队列仍独立。旧四项仅接受后丢弃，不参与新运行快照。

- 旧配置缺字段或 override 为 `null` 时继承默认；显式 `false` 不能被默认合并覆盖。
  零不表示无限；关闭后保留两个数值，服务端仍校验范围，未知字段仍拒绝。
- 发布必须在提交后。开关、人数、秒数沿 `attempt.request_tuning()` 冻结；
  轮询不能重读 live tuning 给旧请求续时。关闭使普通新请求走原路径；strict 子线程
  仍按其容量池规则、同一超时和共享预算等待根绑定。
- 本功能复用 JSONB，不需要改历史迁移。关闭开关不等于可直接降级旧二进制：
  旧解析器可能拒绝新增字段；降级须定向恢复旧版字段，不能清空全部设置。

## 4. 冻结 Universe 与 Live 事实

`WaitingSelection.universe` 优先来自请求的 `FrozenAccountScope.allowed_account_ids()`，
与官方完整池重载一致。缓存中暂时缺失、停用或不满足模型条件的账号不能缩小这个授权范围；
它们恢复合格后可在缓存失效/等待重查时重新进入候选。请求开始后才导入且不在冻结 scope 中的 ID
仍不能进入这次选择。仅无 scope 的本地诊断/兼容调用保留初始 Store ID 集合。
这不是额外账号准入规则，也不是缓存内容决定授权；每次选择仍重验账号资格和容量。

| 保持冻结 | 重新核验 |
| --- | --- |
| 请求 tuning、排序策略、请求间隔、可选显式总截止 | 账号存在性、enabled、凭据状态/到期、额度、cooldown、模型支持观测 |
| `FrozenAccountScope`、required/native owner、Core 排除集 | Provider 会话排除状态、Redis `in_flight`、live 容量及 publication revision |
| 当前选择的 Universe 和 RR cursor | 返回凭据前的完整账号事实及 runtime credential revision |

- 分组/Key 配置修改遵循已有新请求生效语义；FrozenAccountScope 不是在途实时撤权系统。
  停用、删除、凭据和健康变化仍阻止新的发送，不能借冻结为已失格账号继续放行。
- `load_state` 在选择开始读取一次；轮询用只读 `load_signals`，不推进 RR。
  已入队后主要重读目标，完整池重载用于有界重选及成功前的容量观测。
- live 容量必须使用既有 `AccountConcurrencyHandle`；缺失、失效或不可用时拒绝准入，
  不能回退到旧请求的更高上限或无限容量。该句柄含诊断所需账号，不替代资格检查。
- 上调可在下一轮生效；下调不驱逐执行中请求，新晋升必须满足 live 上限。
  删除后重导复用设备不等于恢复旧账号 ID、旧请求权限或精确 WS owner。

## 5. 选择与成功交付

只有全部非容量条件通过且执行并发不足才是 `Busy`。请求间隔仍分类为
`Blocked(RequestInterval)`，仅允许进入既有等待队列，不允许直接执行。
普通可选等待不接纳 cooldown、停用、失效、无额度、越权、模型不支持和业务排除。
strict 子线程是例外：这些阻塞仍禁止执行，但可以等待根绑定恢复、迁移或过期，见下文。

1. Strict 原绑定并发 Busy 且开启普通等待时进入 sticky 等待。队满或超时默认返回原错误；
   `switchAccountOnQueueFailure=true` 时，普通根请求允许一次不排队的立即重选，成功准入后迁移绑定。
   无可用账号保留原队列错误，不重置账号等待预算；required/native owner 和 strict 子线程不适用。
   关闭普通等待时，普通根请求按官方选择其他合法账号并在准入时迁移绑定。
   仅请求间隔阻塞不额外禁止普通根请求迁移；仍必须先通过间隔的原子准入。
   并发与间隔同时阻塞可进入 Strict 并发等待，入队后保持原共享截止。
   required/native 固定账号和严格子线程仍等待其归属，间隔延后不重开预算。
2. fallback 前必须先尝试可立即执行的合法账号。仅调用
   `select_for_capacity_wait` 不证明已经满足“全忙”；Provider 负责完成此判断。
   可选 Preferred 模式先尝试 Ready 候选，不先进入软绑定的 sticky 队列；
   required/native 硬绑定不变，不能借 Preferred 绕过所属账号等待。
3. fallback 一次只持有一个账号的等待租约。无 Ready 候选时，Core 使用原有排序
   从 Busy 或仅请求间隔阻塞（可同时并发满）的候选中选择。使用既有 fallback
   人数上限、共享截止和 Store 原子晋升，间隔未到不算已获取执行槽。
   排队期间其他请求推迟间隔或占满并发时，继续持有原等待租约和截止，不能重新计时。
   无法在窗口内满足间隔时，按原等待超时退出；不沿用软 sticky 的提前放弃窗口逻辑。
   队列满可有界扫描其他 Busy/间隔候选，
   可移动等待者每 500ms 重查完整冻结候选池，即使本账号尚未轮到队首也可检查其他账号。
   找到可用账号仍必须通过目的账号/容量池的原子队首和容量检查；Shared 与 Reserved
   各自 FIFO，不能插队。已进入严格并发等待的绑定与 required/native owner 不跨号。
4. 目标失格或终检冲突先释放所有权再重选。真实账号事实/凭据快照冲突最多重试三次（含首次共四次），
   耗尽返回 `AccountSnapshotChanged`；普通缓存、容量 publication 或亲和重查不消耗这个计数。
   排队中观察到的 strict 根绑定迁移不消耗快照冲突预算，但仍受同一共享截止限制。
   lease-race Busy 集与业务排除集分开，不能伪造 `in_flight` 或写入账号失败记录。
5. 容量轮询保持 100ms，并加 0..24ms jitter；间隔轮询取当前轮询周期和剩余
   间隔的较小值。不能按执行租约 TTL 一次睡数分钟。
   每次晋升前检查目标资格、排除状态和 live limit。

晋升后 `finish_wait_selection` 再核对完整账号事实、capacity revision、
实际 `in_flight` 和 `load_runtime_credential` 一致性。冲突释放执行 guard 并重选，
不得返回旧凭据。终检忽略本请求刚写入的 `last_started_at` 所造成的自身间隔阻塞；
真正准入仍由 Redis 原子检查前一次请求间隔。

所有可失败资格/容量检查在成功亲和 mutation 前完成。准入亲和 CAS（observed-miss 或 observed-generation）输给已有 owner
时释放槽并重选，不能让未发送请求抢走已有绑定。最终容量观测不包含等待人数；
Redis `in_flight` 已含本请求时不得再加一。PG 与 Redis 之间不是跨库强事务，
发送前仍依赖既有凭据、出口 revision 和传输所有权检查收敛竞态。

### Strict 子线程根账号等待

- 对照官方 `74f749f4` 的 follow-only 行为：根账号失效、冷却、停用、被本次请求排除
  或暂不可选时，子线程有界等待，不能直接选择其他空闲账号。普通等待关闭也保留此行为。
- 根绑定改变时先释放旧等待租约，重新读取完整冻结候选集合，再跟随新根账号；不得在
  已按旧根筛过的单账号列表里继续找新根。根绑定消失时可用原有 CAS 重新认领合法账号。
- 根绑定迁移、续期和重选不能重新计时。选中后再核对绑定代次，竞态丢失须释放执行槽。
- 账号重新登录不自动清除 Core 的本次失败排除。已被该请求排除的旧账号继续不可发送；
  可等待根迁至其他合法账号，不能伪造凭据恢复证据或绕过 native owner。
- 队满、超时或跟随选择失败标记 retry prohibited，不能借 provider fallback 逃离根绑定。
  required、Native/ReplayOwner 和诊断指定账号不套用这层 SessionBound 包装。
- 仅持有等待槽不算执行并发、不发送上游请求、不改账号健康。权限、模型、额度、出口、
  指纹、保留并发池及执行租约的原合同保持有效。
- 回归入口 `provider::contract::affinity_recovery` 覆盖普通等待开/关、失效/停用恢复、
  实际根迁移后完成 SSE、失败排除、队满/超时、连续迁移共享截止、冻结池、终检竞态和租约释放。

### 本地损坏凭据候选

`load_runtime_credential` 返回 `InvalidCredentialData` 时，仅未固定账号的选择
可释放当前执行 guard、将 ID 加入本次选择排除集后继续。必须在亲和 claim/renew
之前检查；不写账号健康、不重新读取全部凭据、不增加上游调用。
普通 Ready、sticky Ready 重取及等待晋升三条路径都遵循同一边界。
等待路径以 `WaitOutcome::Skipped` 区别本地坏候选与真实 facts/revision 冲突：
前者消耗一个候选，不消耗快照冲突重试次数，也不重置共享等待截止。
`reload_wait_exclusions` 合并此局部排除集；下一次请求仍重新核验。

required、Native、ReplayOwner、管理员指定诊断及 Store/revision 故障保留原错误，
不能借坏凭据跳过来放宽 owner、账号权限或改用其他账号。
所有候选损坏返回原有无可用账号错误，不自动停用或持久化冻结。
回归必须覆盖多于重扫预算的坏候选、sticky/fallback/Ready 竞态、
晋升前后取消、租约精确释放、亲和未写入及账号健康不变。
错误做法是把每次坏凭据晋升当作 `Retry`；正确做法是释放并跳到下个合法候选。

## 6. 请求共享 Deadline

### 下游 Key 前置排队

Key 并发队列与上游账号容量队列独立。`maxWaitingPerKey=0` 默认关闭，
`keyQueueTimeoutSeconds=30`（1..120 秒）；Key 与账号从各自首次入队分别计时。
Key 等待不压缩账号窗口。账号层跨 attempt 和 Provider 仍共享截止，两层均受请求总截止约束。

Key 位置是进程内有界 FIFO（单 Key 配置上限、所有 Key 合计 1024），Redis
仍拥有并发及 RPM 原子准入。只有队首可以获取并发，拒绝不得记 RPM；
等待中的后继由前序 Drop 唤醒，队首读取 Redis 保留 100ms 间隔。多实例不承诺全局 FIFO。
预算和 RPM 拒绝不能转成等待，也不能提前提交 HTTP/SSE 成功。

Core 在调用准入端口前 arm 清理 guard；明确拒绝才 disarm。未知结果或取消
通过 `BufferedClientAdmissionPort` 的既有 4096 有界队列执行
`cancel_admission`，不逐请求 spawn 新后台任务。Redis 先写取消 tombstone，
再删除对应 active member；迟到 acquire 拒绝该请求 ID。队列满、停机或 Redis
故障遵循原 writer 的告警和 TTL 收敛，不承诺立即释放。

`engine/key_wait.rs` 和 `engine/capacity_wait.rs` 的私有状态单测只在精确
`cfg(test)` 的私有 `tests` 模块内，架构检查按所有者文件明确列举，不允许
其他生产模块添加测试入口或条件导出。

`CoreSession` 创建一次 `Arc<AccountWaitBudget>`，全部 attempts 复用。设：

```text
D_shared = min(D_request, first_wait_start + max(sticky_timeout, fallback_timeout))
D_mode   = min(D_shared, first_mode_start + mode_timeout)
D_active = min(D_shared, 所有已经启动的 D_mode)
```

每个窗口只创建一次；保留已启动模式的截止，不只计算当前模式。任一窗口到期或显式
`expire()` 后预算耗尽。原号等满 120 秒不能再送 30 秒，fallback 到期不能换号重开；
成功后上游执行经过的时间也不暂停窗口。后续 retry 若需要排队，只能使用原窗口剩余时间；
等待耗尽不禁止请求期限内对立即空闲账号的准入，也不独立禁止官方允许的恢复。

Core 使用单调时钟保留截止，并从同一次 wall/monotonic origin 导出 Store 所需绝对时间。
Provider `WaitControl::run` 将取消和 deadline 覆盖到账号读取、Store await、轮询和终检；
取消优先赢得竞速时不得再发业务请求。普通模型请求不设隐式总截止；管理诊断保留
显式 600 秒上限。不存在请求总截止时，Key 与账号等待仍受各自既有有限共享窗口约束，
不能把取消总执行上限解释为无限排队。连接/空闲超时及下游取消语义不变。

## 7. 原子所有权与有界清理

- Client Key admission、账号等待 lease、账号执行 lease、WS 实体槽是不同资源。
  等待不重新消耗一次请求准入，不计入 execution `in_flight`、Smart load 或 used slots。
  先拿账号执行槽，再进入原 WS pool；不能持有 WS socket 反过来等账号槽。
- 同账号、同容量池的 sticky/fallback 共用 waiting ZSET；普通池按配置的单账号人数限制。
  Reserved 按官方 High 优先级免普通单账号人数限制；两池各自仍受每 Provider、每进程
  合计 1024 个等待者约束。执行不占此等待名额，晋升/取消/错误释放名额。
  生产 `bundle` 每实例构造一次 coordinator；1024 归该队列实例所有，测试中的独立 coordinator 模拟另一实例。
  Redis 继续负责跨实例单账号原子准入及 FIFO，1024 不是集群全局上限。
- Store 使用 Redis TIME、唯一 token、绝对 member deadline；key TTL 覆盖最晚 member，
  新入队不能续旧 token。600 秒是异常回收租约，不是普通模型请求总执行上限。
  已交付的普通或晋升执行 guard 每 200 秒续自己的 live member；显式截止仍限制续期。
  续期使用 Redis TIME 和 ZADD XX，不更新 RPM、last-started、亲和或排队人数；
  过期、释放、取消的 token 不得复活。续期暂时失败仅在已知有效期内重试，
  确认所有权丢失或超过该有效期须取消执行，不能继续无并发约束地请求上游。
  Client Key admission 同样随请求 guard 续期；PG recovery 仅续运行中且未过期记录，
  不修改终结记录、不创建缺行；观测失败不独立取消客户端执行。
- 准入前建立 pending ownership；晋升 future 创建时转移 ownership，不能等收到 grant
  才建 guard。即使 future 未 poll、Lua 已提交但响应丢失，也必须有精确 token 可清理。
- 晋升脚本使用与原执行 key 相同的 hash tag，在一次原子操作中检查 token、取消标记、
  截止、执行容量及请求间隔，写执行 member/fence/last-started 并移除等待 member。
  `Busy` 保留等待 ownership；`Expired` 不能伪装成 Busy 无限轮询。
- 成功交付执行 guard 与等待 guard disarm 之间不留 await；迟到的等待释放不得删除已交付槽。
  正常退队显式 await `release`，异常和取消交给 Store 的幂等清理。
- 取消脚本先写 token tombstone，再删精确 waiting/active member；准入和晋升拒绝该 token。
  不能仅早到 `ZREM` 后允许迟到 `ZADD` 复活，也不能用共享裸计数的 `DECR` 清理。
- `Ownership::Drop` 使用有界 `try_send`，不新建无跟踪后台任务。当前清理队列上限 4096、
  并行清理上限 32、单次 I/O 250ms、重试间隔 100ms；重试受相关 lease 最新可能到期时间约束。
  释放/Drop 先停止续期，再提交精确 owner 清理，重试须覆盖已续期的保守期限。
  可能已晋升的结果不确定操作须覆盖 execution 到期，不能仅覆盖较短的 waiting 到期。
- 满队列、断连或超出 drain 上界时保留告警及 TTL 保守回收，不能声称立即清零，
  也不能绕过并发限制。TTL 不是 Redis 数据丢失后恢复全部在途执行状态的保证。

**停机不变量（官方 `ff1ea921`）：** `SchedulingLeaseCleanup::acquire()` 的读闸门
覆盖真实 Redis 直接申请/晋升直到 `track(cancellation, release)` 登记。
`Ownership::supervise` 将精确 owner 与续期任务移入监督释放 future；调用方 guard 仍持有时，
停机也会取消请求、停止续期并释放本实例的执行 member。

监督器接入原有 `CapacityWaitCleanupWriter` 与 Store bundle/worker。停止后不接受新
申请或晋升，等待已发出申请登记，并在官方 2 秒预算内 drain 已登记执行和原清理队列；
owner 全归还可提前结束，超时保留告警和 TTL 回收。未归还等待 guard 不会阻止 worker
无限退出；显式 `release` 仍可执行。Host 的 `worker_shutdown_timeout` 是外层硬截止，
可以更早 abort/join，不因本规则改动 HTTP drain 或新增用户配置。

| 校验场景 | 结果 |
| --- | --- |
| 直接准入/等待晋升后仍持有 guard | 停机取消请求并清理本实例执行租约 |
| Redis 已写入，响应尚未返回 | 等待申请闸门完成登记后清理，不漏掉已占槽 |
| 另一实例仍持有同账号租约 | 不删除另一实例 member，不全局清计数 |
| Redis 不可用或关闭预算已耗尽 | 告警并依 TTL 回收，不能宣称立即清零 |
| 停止后新申请/晋升 | 拒绝，不再发出新占槽写入 |

正常/基线/反例：正常停机无需调用方先 Drop 才清理执行；正常 Drop 仍由同一 owner
释放；反例是复制监督器却没有接到实际 Redis 容量入口，或为了旧测试恢复无限 drain。

必要回归：`shutdown_releases_held_direct_and_promoted_leases_only_for_this_instance`、
`shutdown_waits_for_committed_execution_reply_before_finishing_cleanup` 和
`capacity_wait_shutdown_is_bounded_and_releases_execution_before_waiters_return`，使用真实
隔离 Redis，覆盖 Shared/Reserved、直接/晋升、未返回响应与他实例。
错误做法是把“Host 有外层截止”推导成 Store 不能有官方自己的 2 秒释放预算；
正确做法是两个边界各司其职，超时明确依 TTL 兜底。

## 8. 错误、Native 与协议边界

| 实际原因 | Provider / Gateway 分类 | HTTP |
| --- | --- | --- |
| `queue_full` | `ConcurrencyQueueFull` | 429，`concurrency_queue_full` |
| `wait_timeout` | `ConcurrencyQueueTimeout` | 429，`concurrency_queue_timeout` |
| `wait_token_expired` / 真实账号容量不可用 | `AccountCapacityUnavailable` | 503，`account_capacity_unavailable` |

三者均为本地 `NotSent`；Store 故障保留基础设施错误，不能机械改成 429。
等待不制造上游 attempt、熔断观测或 SSE 首包。`QueueRejection::provider_kind()`
同时用于 xAI；API 通过 `GatewayError::from_provider` 保留分类。

Native prepare 恢复按官方完整分支：只处理未发送时的无候选、容量不可用或额度耗尽，
要求仍有换号预算和 Provider 会话状态；required account、retry prohibited 仍禁止换号。
队满/超时不直接进入此 Native 恢复分支。流内的 exact-owner/client-replay disposition
仍按独立流内恢复逻辑处理，不能拿它给所有 prepare 阶段额外上锁。
跨 Provider 候选推进独立检查未发送状态、续写模式、retry prohibited 和换号预算，
不能仅因账号排队预算到期而禁止推进。普通 retry 的 `account_state_owner` 本身不构成硬绑定。

严格根会话队满/超时后的重选仍由既有开关控制，释放队位后仅立即尝试一次，取得租约
并成功提交亲和 CAS 后绑定新账号。实际迁移记录 `account.binding.migrated` 及原因、前后账号；
失败准入/未改绑定不记迁移。strict 子线程仍跟随根绑定，不能绕过它独自重选。

等待成功继续走原 HTTP/WS 执行入口。精确续接保留原账号、socket、出口和画像；
不得删除 `previous_response_id`、伪造 history loss 或改用 HTTP 绕过等待。
API 不为账号排队提前提交 SSE 或发送业务 payload。

## 9. 必须保留的回归

### Guardian 独立池

- `ProviderConcurrencyPool::{Shared, Reserved}` 沿状态读取、原子执行准入、
  等待压力、入队、晋升及终检一致传递；不再用普通上限减去预留数。
  `R>0` 的 Guardian/review 用 R，普通请求用 L；R=0 回到 Shared。
- Core `AccountSelectionContext::limit_for_pool` 统一冻结/live 容量与诊断上限；
  即使 R 独立，live publication 缺失也不准入。普通容量观测端口只读 Shared。
- Redis 只在原账号 active key 后追加 `:reserved`，独立 waiting/order/cancelled；
  间隔和 fence 仍是账号级。Quality 继续独立 `:quality`，不进入审批池。
- Shared 和 Reserved 排队各自用 fence 序列保留 FIFO，deadline ZSET 负责到期；
  不能按截止早晚排序。直接执行和跨账号重查同样原子复核目的队首，
  取消/超时同时移除排序位置；Quality 不进入业务等待队列。
- 普通池保留单账号人数限制，Reserved 免此限制；两池均接入官方每进程等待总量 1024。
  普通请求在等待开关关闭时不隐式排队；strict 子线程跟随根账号时保留上述有界等待。
  容量、续期和不确定结果清理仍复用既有所有权流程。
- 未实现 Reserved 的端口失败关闭，不得回落读取普通池的计数。

| 层 | 最低反例与断言 |
| --- | --- |
| 配置 | 旧 JSON/null/false、统一人数/时限默认及往返、关闭保值、非法值不提交；POST 到 PG/snapshot/新请求；旧请求仍冻结 |
| Core | Busy 与双阻塞区分、全失格不等、Busy+失格仍可等；Smart/权重不变；跨 attempt/模式共享截止 |
| Provider | 原号优先、全忙才 fallback、队满有界退出；等待内导入新 ID 不扩 Universe；disable/delete/额度/模型/排除变化终检 |
| Live 容量 | 上下调及 publication 竞态、缺失失败关闭、凭据 revision 冲突归还槽；容量观测不计 waiter、不重复计算自身 |
| Redis | 普通池单账号上限、Reserved 免此上限、每 Provider/池/进程总量 1024、晋升/取消释放名额、原子晋升不超卖和独立 member TTL |
| 取消 | 准入/晋升前中后取消、未 poll future、成功响应丢失、取消先到、重复 release、旧 token 不删新 owner |
| 停机 P1 | 持有执行 guard、申请响应未返回、本实例与他实例隔离、官方 2 秒预算；Host 外层更短截止仍可 abort/join，失败依 TTL |
| Native/协议 | 队满/超时不进入 Native 跨号重放；本地无候选等仅按官方 prepare 分支恢复；耗尽等待但立即空闲可准入，换号预算为零仍禁止恢复；等待时零上游调用且无提前 SSE |

对应回归位于 Core `tests/account/selection.rs`、`tests/engine/capacity_wait.rs`、
`tests/engine/coordinator.rs`，Store `tests/redis/capacity_wait.rs`，以及 OpenAI
`tests/credential/contract/mod.rs`、`tests/provider/contract/scheduling.rs` 等受影响测试模块。
表格是验收要求，不表示每行已获实证。真实隔离 PG/Redis 不可用导致的 skip 不能计为通过；
按影响执行验证并复用有效证据；定向测试不代表完整协议、生命周期或性能验收。

## 请求间隔修复回归

- `credential::contract::capacity_wait_fallback_interval_rechecks_and_releases`
  覆盖未绑定请求的间隔、并发叠加/切换、竞争推进、队满、超时、取消和账号失格。
- `capacity_wait_fallback_interval_preserves_ready_priority_and_opt_out` 确认先用 Ready
  候选，关闭开关不入队；`capacity_wait_fallback_interval_serializes_a_burst_without_requeueing`
  确认并发突发仍遵守每次真实发送间隔，不重新入队续期。
- `credential::contract::pinned_interval_wait_preserves_owner_and_releases_on_cancel_or_full`
  覆盖固定账号就绪、取消、队满、停用、间隔推进、并发叠加与共享超时；所有分支等待租约归零。
- `provider::contract::affinity_modes::strict_child_waits_for_root_request_interval_without_switching_accounts`
  从公开 Provider 入口建立根绑定，再让子线程遇间隔，确认只使用根账号且不创建独立绑定。
- 原软亲和短间隔、过长间隔、质量检查专用等待、Native owner 及容量限制回归必须继续通过。

## 官方 v3.41.2 增量约束

- 严格模式已有主绑定因并发满且开启等待时，只等待原账号；关闭普通等待允许改选并迁移。
  仅间隔阻塞不锁定主账号，进入 fallback 后仍可发现新空闲账号；required/native 和子线程归属不变。
  Core 允许错误恢复且剩余换号预算足够时，排除旧根账号后可选新账号并在发送前 CAS；
  请求完成不写主绑定。严格子线程沿 follow-only 等根恢复/迁移。
- RootChanged 不消耗 snapshot conflict 重试预算，但必须清理旧 sticky_tried 集合、
  重读完整冻结候选和当前绑定并保留原 deadline；A→B→A 不能误入一般 fallback。
- 排队全池复查复用官方 scheduling projection/scoped reads；缓存不替代 live 容量、
  Redis 队首、credential revision 与取消检查。源等待租约释放后才能交付新执行租约。
- 对应实现与逐官方文件差异见 `docs/upstream-v3412-native-parity.md`，运行验证结果
  必须按实际测试记录，不能仅凭本文宣称通过。

无显式会话的缓存偏好同样服从上述模式。Preferred 的可选偏好存储故障必须在
直接、投影和等待路径均停用本次偏好；已持有的等待/执行槽按正常生命周期释放。
真实会话和 Strict 缓存偏好不降级，CAS 冲突不属于可忽略的存储故障。


## v3.42.0 原生接入

- 对外写入 `keyQueueTimeoutSeconds` / `accountQueueTimeoutSeconds`，旧两个字段名仅作为输入 alias；两者范围均为 1..120 秒。迁移0086将已保存的超范围值限至120秒，保留人数、显式关闭和其他设置。
- `switchAccountOnQueueFailure` 默认 false。开启后只在队满/超时后进行一次立即重选，不重新排队；严格根请求取得 B 后绑定 B，Preferred 保留原软绑定。固定账号、真实续接 owner 和严格子线程不跨号。
- xAI 接入官方进程内等待队列及重选 helper；执行并发继续由已有 Store 准入拥有，不承诺跨实例的全局等待 FIFO。OpenAI 保留既有 Redis 等待 owner 和 Shared/Reserved 容量池。
- 官方回归与本地队列接线同时验证；固定官方来源、差异及结果见 `docs/upstream-v3420-native-parity.md`。

## 10. 典型正反例

- 正常：等待已耗尽、请求未过期，新 attempt 的合法候选立即空闲，仍可准入；若还需排队则返回 429 超时。
- 边界：Reserved 同账号已有一个等待者且普通人数配置为 1，第二个仍可入队；该进程同 Provider/池已有 1024 等待者时拒绝。
- 错误：把任意三次缓存/容量重查压成 coordinator 故障；正确做法仅对真实快照冲突计三次重试，其余受取消和截止约束。
- 保留官方风险：strict 子线程等根绑定时可占据 FIFO 队首，执行槽空闲也可能队满。429 分类对齐不等于消除这种阻塞；改变该机制须独立授权、设计和验证。

## 管理员额度暂停（v3.45.1）

`quotaLimits` 为账号通用窗口 1..100 已用比例上限，空对象关闭；`quotaPaused` 与上游耗尽、
临时限流独立。任一受限窗口达到上限即暂停，恢复须所有受限窗口都有明确低于上限的证据。
缺失窗口保留既有确认，不能靠元数据覆盖解除暂停。写入检查 limits 和 merge-base CAS；
保留本地请求起始/捕获时间、credential revision 和旧成功回写保护。

即时候选、队列重扫、等待目标、提升前和取得租约后的重读均复核暂停。
Strict 子线程跟随根 owner，owner 暂停返回 RequestPolicyDenied，不偷偷借其他账号；
Preferred/Relaxed 在自身权限及原换号预算内可选择健康候选。固定账号和真实续接 owner
仍遵守原归属。既有质量复测可诊断暂停账号，普通业务的额度透支不能绕过管理上限。
保存/清空限制使用当前额度重算；正常 token refresh 保留额度证据，新授权清除旧身份额度。
