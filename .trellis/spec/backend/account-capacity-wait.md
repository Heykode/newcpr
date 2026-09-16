# 账号容量等待合同

## 1. 范围与证据边界

适用于 OpenAI 正常请求在账号执行并发不足时的可选等待。等待不是 WS socket
排队，不是新的调度算法，也不增加执行并发。管理诊断走原 selector 路径；
不能据此要求 OAuth 刷新、后台额度/目录任务或其他 Provider 自动排队。

本文根据任务 `09-14-qx-profile-capacity-wait` 的当前 Core、OpenAI Provider、
Redis Store 实现及研究合同同步。本轮已通过全 workspace、真实 PG/Redis、Host 停机
监督和界面验证，证据见该任务 `research/integration-verification.md`。
维护合同不代替未来改动的验证，也不代表性能压测、跨平台发布或生产验收完成。

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
| `accountBusyWaitStickyMaxWaiting` | `3` | 整数，`1..=1000` |
| `accountBusyWaitStickyTimeoutSeconds` | `120` | 整数，`1..=600` |
| `accountBusyWaitFallbackMaxWaiting` | `100` | 整数，`1..=1000` |
| `accountBusyWaitFallbackTimeoutSeconds` | `30` | 整数，`1..=600` |

- 旧配置缺字段或 override 为 `null` 时继承默认；显式 `false` 不能被默认合并覆盖。
  零不表示无限；关闭后保留四个数值，服务端仍校验范围，未知字段仍拒绝。
- 发布必须在提交后。开关、人数、秒数沿 `attempt.request_tuning()` 冻结；
  轮询不能重读 live tuning 给旧请求续时。关闭只使新请求走原路径。
- 本功能复用 JSONB，不需要改历史迁移。关闭开关不等于可直接降级旧二进制：
  旧解析器可能拒绝新增字段；降级前须备份并定向处理五个 key，不能清空全部设置。

## 4. 冻结 Universe 与 Live 事实

`WaitingSelection.universe` 是一次 selector 调用首次 `list_for_provider()` 得到的
账号 ID 集合。该次等待及有界重选中的池重载只能取这个集合与冻结
`FrozenAccountScope` 的交集。即使 scope 是 AllAccounts，等待期间新导入的 ID
也不能加入这次选择。不要将这个局部集合误称为跨所有 attempt 的新全局目录。

| 保持冻结 | 重新核验 |
| --- | --- |
| 请求 tuning、排序策略、请求间隔、总截止 | 账号存在性、enabled、凭据状态/到期、额度、cooldown、模型支持观测 |
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

只有全部非容量条件通过且执行并发不足才是 `Busy`。请求间隔、cooldown、
停用、失效、无额度、越权、模型不支持和业务排除都是 `Blocked`；
“并发满且间隔未到”不能因检查顺序被误判为可等待。

1. 合格原绑定账号仅 Busy 时优先 sticky 等待。软亲和队列满可转正常选择；
   required account、Native、ReplayOwner 的硬绑定不得自行换号。
2. fallback 前必须先尝试可立即执行的合法账号。仅调用
   `select_for_capacity_wait` 不证明已经满足“全忙”；Provider 负责完成此判断。
3. fallback 一次只持有一个账号的等待租约。队列满可有界扫描其他 Busy 候选，
   成功入队后固定目标，不承诺严格 FIFO 或其他账号空闲后立即迁移。
4. 目标失格或终检冲突，先释放所有权再有界重选；当前外层最多三轮。
   lease-race Busy 集与业务排除集分开，不能伪造 `in_flight` 或写入账号失败记录。
5. 轮询从 100ms 退避至 1s，并加 0..24ms jitter；不能按执行租约 TTL 一次睡数分钟。
   每次晋升前检查目标资格、排除状态和 live limit。

晋升后 `finish_wait_selection` 再核对完整账号事实、capacity revision、
实际 `in_flight` 和 `load_runtime_credential` 一致性。冲突释放执行 guard 并重选，
不得返回旧凭据。终检忽略本请求刚写入的 `last_started_at` 所造成的自身间隔阻塞；
真正准入仍由 Redis 原子检查前一次请求间隔。

所有可失败资格/容量检查在成功亲和 mutation 前完成。初始亲和 CAS 输给已有 owner
时释放槽并重选，不能让未发送请求抢走已有绑定。最终容量观测不包含等待人数；
Redis `in_flight` 已含本请求时不得再加一。PG 与 Redis 之间不是跨库强事务，
发送前仍依赖既有凭据、出口 revision 和传输所有权检查收敛竞态。

## 6. 请求共享 Deadline

### 下游 Key 前置排队

Key 并发队列与上游账号容量队列独立。`maxWaitingPerKey=0` 默认关闭，
`keyConcurrencyWaitTimeoutSeconds=30`；首次 Key 入队将自己的单调截止时间
写入同一 `AccountWaitBudget`，随后所有账号等待和 attempt 不得延长该截止。
未发生 Key 等待的请求保持原账号等待窗口行为。

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
成功后上游执行经过的时间也不暂停窗口，retry 只能使用原窗口剩余时间。

Core 使用单调时钟保留截止，并从同一次 wall/monotonic origin 导出 Store 所需绝对时间。
Provider `WaitControl::run` 将取消和 deadline 覆盖到账号读取、Store await、轮询和终检；
取消优先赢得竞速时不得再发业务请求。请求总截止仍受既有 Core 生命周期约束。

## 7. 原子所有权与有界清理

- Client Key admission、账号等待 lease、账号执行 lease、WS 实体槽是不同资源。
  等待不重新消耗一次请求准入，不计入 execution `in_flight`、Smart load 或 used slots。
  先拿账号执行槽，再进入原 WS pool；不能持有 WS socket 反过来等账号槽。
- 同账号 sticky/fallback 使用同一个 waiting ZSET 与总 `ZCARD`，各自阈值检查该总数，
  不是独立的 3+100 保留区，也不是每个 Client Key 各 100 人。
- Store 使用 Redis TIME、唯一 token、绝对 member deadline；key TTL 覆盖最晚 member，
  新入队不能续旧 token。执行到期继续受请求总截止及原有 600 秒上限限制。
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
  并行清理上限 32、单次 I/O 250ms、重试间隔 100ms；重试受相关 lease 原到期时间约束。
  可能已晋升的结果不确定操作须覆盖 execution 到期，不能仅覆盖较短的 waiting 到期。
- 满队列、断连或超出 drain 上界时保留告警及 TTL 保守回收，不能声称立即清零，
  也不能绕过并发限制。TTL 不是 Redis 数据丢失后恢复全部在途执行状态的保证。

**停机不变量：** Store worker 进入 stopping 后拒绝新的所有权/
晋升，继续接受仍存活请求及执行 guard 的迟到 Drop。owner 归零后才关闭 receiver，
排空输入队列和正在清理的 future；不能在收到共享取消信号时立即关闭，也不能另设 35 秒
或其他按默认 HTTP drain 推算的固定截止。
Host 是停机时间的唯一权威：配置的 HTTP drain 结束后，既有 supervisor 使用统一的
`worker_shutdown_timeout` 绝对截止等待所有任务，逾期 abort 并 join。Store 不复制
Host 预算、不新增用户设置；保留队列/并发上限、单次 I/O 超时和原 lease 到期约束。
daemon 单独 `run()` 不承诺自行限时退出，独立调用方须提供外层 timeout/abort。
Host 强制终止后的残留依原 TTL 保守回收，不承诺立即释放全部槽。

## 8. 错误、Native 与协议边界

`queue_full`、`wait_timeout`、`wait_token_expired` 映射为
`AccountCapacityUnavailable + NotSent` 的安全诊断；存储/必要事实不可用必须失败关闭，
不能当作人数为零。等待不是 upstream attempt，不制造上游失败、熔断观测或 SSE 首包。

启用路径硬绑定错误通过 `PinnedAccount` 携带恢复限制：owner 未丢失时
`RetryExactConnection`，选择已无合格 owner 时 `ClientReplayRequired`。
Core 必须尊重这些 disposition 及耗尽预算，不能因为本地容量错误进入 `ReplayAny`。
这不是禁止所有经 Provider 证明安全的重放；关闭路径原有恢复合同仍保留。
普通 retry 的 `account_state_owner` 本身不构成 Native 硬绑定。

等待成功继续走原 HTTP/WS 执行入口。精确续接保留原账号、socket、出口和画像；
不得删除 `previous_response_id`、伪造 history loss 或改用 HTTP 绕过等待。
API 不为账号排队提前提交 SSE 或发送业务 payload。

## 9. 必须保留的回归

| 层 | 最低反例与断言 |
| --- | --- |
| 配置 | 旧 JSON/null/false、四数值默认及往返、关闭保值、非法值不提交；POST 到 PG/snapshot/新请求；旧请求仍冻结 |
| Core | Busy 与双阻塞区分、全失格不等、Busy+失格仍可等；Smart/权重不变；跨 attempt/模式共享截止 |
| Provider | 原号优先、全忙才 fallback、队满有界退出；等待内导入新 ID 不扩 Universe；disable/delete/额度/模型/排除变化终检 |
| Live 容量 | 上下调及 publication 竞态、缺失失败关闭、凭据 revision 冲突归还槽；容量观测不计 waiter、不重复计算自身 |
| Redis | 3/100 共享人数、跨 Key 同账号共享、账号隔离、原子晋升不超卖、独立 member TTL |
| 取消 | 准入/晋升前中后取消、未 poll future、成功响应丢失、取消先到、重复 release、旧 token 不删新 owner |
| 停机 P1 | 超过旧 35 秒窗口后请求/执行 guard 才 Drop 仍可清理；存活 owner 未归还不能提前关闭；Store 验证外层 timeout/abort，Host 验证自定义 HTTP drain 后统一限时 abort/join |
| Native/协议 | full/timeout/失格不进入 ReplayAny；关闭路径不变；等待时零上游调用且无提前 SSE；取得执行槽后真实 WS 及精确续接 |

对应回归位于 Core `tests/account/selection.rs`、`tests/engine/capacity_wait.rs`、
`tests/engine/coordinator.rs`，Store `tests/redis/capacity_wait.rs`，以及 OpenAI
`tests/credential/contract.rs`、`tests/provider/contract/scheduling.rs` 等受影响测试模块。
表格是验收要求，不表示每行已获实证。真实隔离 PG/Redis 不可用导致的 skip 不能计为通过；
定向测试不能代替 workspace、协议、生命周期和性能验收。
