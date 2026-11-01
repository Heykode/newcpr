# 运行设置单位与 Fast 对齐

官方核对基准：`zyycn/codex-proxy-rs@fbdfe22757cc8a4057cf598005fdac58e0ca5a18`。

## 本次实现

| 项目 | 行为 |
| --- | --- |
| 解压上限、大请求 HTTP 阈值 | Web 以 MiB 展示和输入，API/数据库仍保存整数字节；保留旧值精度 |
| 请求间隔、WS 寿命/空闲/故障窗口/暂停 | Web 以秒输入，支持小数；内部继续使用毫秒 |
| Excel 提交前缓冲 | 原控件移到 Excel 菜单，继续使用 KiB；原生 Codex 不读取这一缓冲阈值 |
| 全局关闭 Fast | 删除 Web/API/Store/Snapshot/Plan/AttemptContext/Provider 的全局覆盖路径 |
| 分组 Fast | 沿用已具备的 default/enabled/disabled；Disabled 优先；不修改账号范围和其他档位策略 |

本次没有更改 UA、地区覆盖、排队、重试、冷却或模型映射。没有执行生产设置更新。
新增迁移 0081 仅删除 runtime_settings.disable_fast；分组兼容列保留。
官方对应删除位于 `0016_client_profiles_and_model_pricing.sql`；本项目按现有迁移序列新增。
旧二进制依赖该列，不能直接回滚旧镜像。全局禁用不会转写为所有分组禁用。

## 核对结论

官方 `backend/migrations/0035_cpr_link_and_concurrency_defaults.sql` 把新安装
max_waiting_per_account 改为 10；`0008_concurrency_queues.sql` 的统一等待时间为 30 秒。
`gateway-core/src/settings/compiled.rs` 将这一人数和时限传入账号队列；Key 队列共用时限。
它没有本项目 sticky/fallback 两套四字段，但具有账号等待实现。
`providers/openai/src/credential/selector.rs` 对 follow_only 保留特殊有界等待，
普通等待人数为零时使用 1000 人边界；这不是无条件允许跨账号续接。

2026-11-01 对生产 runtime_settings 的只读查询（revision 3953）：

| Web 字段 | 当前生产值 | 官方对应及新安装默认 |
| --- | --- | --- |
| 原绑定账号等待人数上限 | 5 | maxWaitingPerAccount，10 |
| 原绑定账号最长等待 | 40 秒 | concurrencyWaitTimeoutSeconds，30 秒 |
| 全忙时单账号等待人数上限 | 10 | 同一个 maxWaitingPerAccount，10 |
| 全忙时最长等待 | 60 秒 | 同一个 concurrencyWaitTimeoutSeconds，30 秒 |

人数为等待队列容量，不是正在执行的请求数。官方不是两组各自可配的参数；
旧实例已保存值保持原值，不能把新安装默认值当作所有官方实例的现值。

本项目 `websocketMaxRetries` 是数据库/Web 可覆盖参数；官方启动配置
`OpenAiConfig.stream_max_retries` 默认 5、上限 100。Provider 创建同一恢复状态时
优先选择 Web 快照，否则使用启动值。两者不相加，也不会把 Web 修改写回启动配置。
上述生产快照的 Web 重试值为 1，真实限流兜底冷却为 30 秒。

UA 差异仅审查：官方 `transport/profile/selection.rs` 提供 Desktop/CLI、平台、
架构、Latest/Fixed、版本滞后及配套版本解析。本项目固定第三方样本不是这个机制。
当前 Custom 固定模式不被默认版本自动替换；CLI 前缀和后缀同源更新，HTTP/WS 共用
请求冻结画像。Desktop 自动模式仍复用单个默认 Desktop 发布版本，不能宣称完整平台对齐。
后续可保留当前自定义值、系统版本和客户端版本输入，接入官方按平台解析和预览。
本次未修改这些文件，未把差异解释为已证实的风控或线上故障原因。

## 验证范围

隔离 Linux 执行，禁止本机或生产测试。覆盖单位保存往返、空草稿、精度边界、
Excel 菜单可见性、分组 Fast wire 行为、API 字段移除、真实数据库迁移及其他设置保留。
具体执行结果记录在本地任务验收记录。
