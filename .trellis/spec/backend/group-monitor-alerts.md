# 分组监控通知合同

## 1. 范围

通知仅消费已有十秒分组监控样本。不得调用上游额度刷新、修改账号状态、
调度、指纹、估值、并发准入或账号列表刷新周期。采样先保存，告警评估
最多等待三秒，失败不影响监控读取和健康状态。

## 2. 接口和存储

- `GET /api/admin/notifications/channels`
- `POST /api/admin/notifications/channels/update`
- `POST /api/admin/notifications/test`
- `GET /api/admin/notifications/deliveries?groupId=...`
- `GET /api/admin/account-groups/alert-policy?groupId=...`
- `POST /api/admin/account-groups/alert-policy/update`

全部要求管理员认证。渠道更新仅接受 `smtp`、`bark`；策略更新不得提交
只读 `updatedAt`。数据库迁移 `0037_group_monitor_alerts.sql` 新增渠道、
策略、事件、投递队列四张表。配置写入和管理审计使用同一事务。

## 3. 数据合同

- SMTP 密码和 Bark Device Key 只写，空字符串保持原值。读取和 Debug
  不暴露密钥。`passwordSet/deviceKeySet` 由服务器现有值或真实新密钥决定，
  不信任客户端标记。数据库遵循项目既有凭据存储方式，未新增应用层加密。
- 并发使用 `usedSlots / totalSlots`，默认 90%，持续 20 秒。
- ETA 仅使用 `ready` 且有限的分钟值，默认不超过 10 分钟，持续 30 秒。
- 额度为零和可调度账号为零默认立即触发。删除最后一个账号仍可触发。
- 未知数据不触发，也不构成已确认恢复。预计过期额度从不触发告警。
- 每组每条件一个事件；激活后不重复入队；连续恢复 30 秒重新允许通知。
  未激活的确认过程一旦恢复立即重置。拒绝旧采样覆盖较新状态。
- 采样中断超过 30 秒时，重置未完成的确认或恢复计时。间隔按整秒比较，
  避免 PostgreSQL 微秒精度使恰好 30 秒的恢复样本被误判为中断。
- 创建事件行后再加锁，固定组/条件顺序处理，防止空行锁失效导致重复投递。
- 每个收件人和 Bark 各一条 outbox；五分钟回收失联 claim，失败一分钟后
  独立重试，最多五次。旧事件恢复后停止未发送的旧通知。
- 测试通知原子插入为 `sending`，直接发送自己的 ID，失败不重试，也不创建事件。
  分组测试使用本组 Bark 样式覆盖。
- 传输有界：SMTP 总超时 20 秒；Bark 10 秒且响应最多 16 KiB，禁止重定向。
  Bark POST 使用服务地址下的 `/push`，不把密钥拼到 URL。
- 对端已收到但本地未成功落库的异常中断仍可能重发，不能承诺跨 SMTP/Bark
  的严格 exactly-once。

## 4. 校验与错误

| 输入或状态 | 行为 |
| --- | --- |
| 非管理员 | 401 |
| 未知请求字段 | 422 |
| 非法端口、音量、阈值、邮箱、Bark URL | 400 |
| SMTP/Bark 未启用或没有真实配置 | 不创建可投递事件 |
| 发送失败 | 脱敏固定错误，不回传上游响应或凭据 |
| 分组策略或渠道关闭 | 发送前复查，不继续实际投递 |

## 5. 正常与异常例子

- 并发 95% 持续 20 秒产生一次邮件和一次 Bark，之后 95% 不再产生新通知。
- 短暂超过阈值后恢复，重新开始计时，不累计不连续的异常时段。
- 邮件成功、Bark 失败，仅重试 Bark。
- 没有任何通知路由时不激活事件，之后配置路由仍能正常首次告警。

## 6. 验收

真实隔离 PostgreSQL 覆盖迁移、秘密字段、策略保存、多实例首次激活、
重复采样、恢复、旧采样、分目标重试和测试通知。API 覆盖认证、严格 DTO、
伪造密钥标记和全局投递读取。Host 使用本地模拟 SMTP/Bark，禁止真实凭据。
前端测试位于 `group-monitor-alerts.test.mjs` 和 `browser/group-alerts.mjs`。
Admin 阈值测试使用 `tests/use_case/notifications.rs` 中的公开服务入口，Host
传输测试使用 `tests/notifications.rs` 中的 `HostBundle::notification_delivery`。
不得在生产 `src` 内增加测试模块或为测试开放私有接口；必须运行网关应用的
跨工作区架构检查，不能仅验证 Admin、Store、API、Host 包。

## 7. 错误与正确写法

错误：对不存在的事件 `SELECT FOR UPDATE` 后直接入队；多个实例都认为首次激活。

正确：先 `INSERT ... ON CONFLICT DO NOTHING` 固定主键，再锁同一行，状态和
outbox 同事务提交。失败后的状态更新必须包含配置读取和配置校验失败路径。
