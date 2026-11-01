# 数据库迁移

sqlx 在服务启动、监听请求之前按编号执行迁移，并把文件 checksum 记入 `_sqlx_migrations`。
已应用迁移的字节与记录不一致时，服务会拒绝启动。

`0001_initial.sql` 定义数据库初始化基线，后续 schema 变更按编号新增迁移。
完整清单以本目录 SQL 文件和 `.frozen-sha256` 为准。

## 冻结规则

- 同一大版本内，已合入 main 的迁移文件**永久冻结**，一个字节都不能改——包括注释和
  空白。任何改动都会让该大版本的已部署实例在下次重启时因 checksum 失配而无法启动，
  且没有自动恢复路径。
- 同一大版本内的 schema 变更一律新增编号迁移，永远不回改
  旧文件。切换大版本时允许把已完成升级的 schema 折叠成新的 `0001` 基线，因为跨大版本
  不支持在线升级，目标版本必须使用全新数据目录部署。
- `.frozen-sha256` 是冻结清单（`sha256sum` 格式）。CI 会校验：清单内文件
  字节未变，且目录里每个 `*.sql` 都已入册。新增迁移时在同一提交里执行
  `sha256sum 000N_xxx.sql >> .frozen-sha256` 入册。
- 若同一大版本内确需修正已冻结迁移里的错误，用新迁移做补偿性变更（`alter` / 回填），
  不要动原文件。

从迁移目录检查已登记文件：

```bash
cd backend/migrations
sha256sum --check --strict .frozen-sha256
```

CI 还会检查是否遗漏新 SQL 文件，并在 PR 中检查清单只增不改。
遇到 checksum 不一致，先核对运行版本和文件来源；不要修改数据库中的 checksum 来绕过校验。

## 亲和模式升级兼容

`0068_preferred_account_affinity.sql` 仅扩大亲和模式的允许值，保留现有
Strict/Relaxed 设置和 Strict 默认值，不改账号、身份、出口或调度权重。
管理员保存 Preferred 后，旧二进制不能读取这个新枚举；回退必须先停服、
明确恢复兼容模式，并按照迁移感知的部署方案处理，不能直接回滚镜像。

## 本地测试库

0081_remove_global_fast.sql 删除废弃的全局 disable_fast 列，分组 fast_mode 与
其兼容列保留。运行配置和上游请求只使用分组策略；该迁移不修改其他已保存参数。
旧二进制依赖已删除列，升级后不可直接回滚旧镜像，需使用迁移感知的部署流程。

0078_concurrency_defaults.sql 对齐初始化并发/排队策略，保留已保存和自定义设置。
0079_account_plan_snapshots.sql 为新请求和错误事件保存历史套餐，旧记录保持未知。
0080_account_name_bytes.sql 将 custom_name 上限对齐为 512 个 UTF-8 字节，
兼容全部原有合法名称；旧前端可能无法编辑超过其 128 字符限制的新名称。
三项均为新增迁移，不修改历史 checksum，不更改账号身份和出口。

0077_codex_quota_overdraft.sql 增加默认关闭的 Codex OAuth 额度透支设置与
多实例探测状态表。状态表不替代 quota_access_state，探测失败仍由原额度耗尽和
quota_reset_at 控制调度；只有当前配置、凭据和额度周期均匹配的 passed 状态
可以对符合资格的文本请求放宽额度耗尽过滤。旧二进制忽略新状态表，回退前必须
关闭透支开关。

`0074_auto_reset_inventory.sql` 持久化自动重置卡的库存计划；原配置新增字段默认关闭，
不改变现有按额度阈值消费的行为。复用原有兑换操作 ID 和消费台账，不自动换卡重试未知结果。
开启到期用卡或周剩余额度条件后，旧二进制不认识这些配置字段，回退前需处理配置兼容。

`0075_proxy_websocket_capacity.sql` 为普通代理新增 WS 物理连接上限，默认 `0` 不限。
按完整代理 URL（含认证）在单进程合计，不是跨实例总额，也不是账号并发上限。
不更改账号绑定和出口选择。旧二进制不执行此预算，不能视为启用预算后的等价回退。

`0073_group_image_generation.sql` 为分组增加默认允许的生图权限。Key 绑定的任一分组
禁止生图时，Images 与 Responses 生图工具均被拒绝，普通图片输入不受影响。
旧客户端未发送新字段时保留已有权限。初始并发改为 5；启动迁移在锁内确认新建库，
仅新建库应用该初始值，已有库保留并发设置。旧二进制不执行生图权限，不能作为禁用组的
等价回退；本迁移不修改历史迁移、账号身份、累计费用或出口。

`0072_outbound_proxy_enabled.sql` 为保存的代理新增默认开启的开关，不改写账号或代理绑定。
禁用代理保留绑定但停止在运行态使用；删除代理会在管理事务中自动解绑。
旧二进制不识别禁用状态，回退前应处理禁用代理并遵循迁移感知部署流程。

`0082_outbound_client_profile.sql` 新增结构化 UA 配置文档，仅显式选择新预设或
完整自定义模式时使用。旧 default/custom/custom-auto-version 的模式、字符串、
时间戳均不重写。业务备份通过整表 pg_dump 包含新列；启用新模式后旧二进制
不识别该模式，回退须先恢复旧模式并遵循迁移感知部署流程。

`0083_shared_account_wait.sql` 将账号等待的旧四项退休，统一为
`maxWaitingPerAccount=10` 与 `accountConcurrencyWaitTimeoutSeconds=30`。
这是用户明确选择的旧值迁移，不是根据两组旧值猜测优先级。已保存的新两项保留；
等待开关、Key 队列、重试、UA 和其他设置不变。旧二进制不识别新字段，
回退须按迁移感知流程处理，不能仅替换镜像。

`gateway-store` 的 PG/Redis 集成测试需要以下环境变量，未设置时在本地
静默跳过（CI 缺失则直接失败）：

```bash
export CPR_TEST_DATABASE_URL='postgres://<user>:<password>@127.0.0.1:5432/<db>'
export CPR_TEST_REDIS_URL='redis://:<password>@127.0.0.1:6379'
```

测试自建随机 schema / key 前缀做隔离，但仍应使用专用开发或测试实例，不要连接生产库。
凭据从自己的部署配置或 CI Secret 中读取，不要粘贴未脱敏的 `docker inspect` 输出。
环境变量未设置导致的跳过不算数据库测试通过。
