# 账号模型限制

## 1. 范围

人工账号模型政策适配官方 PR 105，不包含自动封禁。修改账号设置、
配置快照、候选筛选、导入导出或前端账号表单时遵循本合同。

## 2. 接口

- `AccountModelAccess { mode, models }` 经校验反序列化，字段不可直接构造。
- 账号设置和批量补丁的 `model_access: Option<AccountModelAccess>`。
- API 字段为 `modelAccess`，账号响应始终返回有效政策。
- `0035_account_model_access.sql` 新增 `provider_accounts.model_access_json`。
- `FrozenAccountScope::allows_model(account_id, upstream_model)` 筛选候选；
  `allows_provider_model` 在整个授权池判断目录可见性。

## 3. 合同

匹配映射后的精确上游模型 ID，不改变评分、粘性键、设备、代理、WS 池键。
普通选择和容量等待都先检查政策，再检查 State、额度及租约。
粘性和指定账号不绕过政策；受管模型没有有效 State 仍不得建立新链。
管理员诊断、无模型的独立 Images/Search 延续既有例外。
单请求冻结政策，不中断已经执行中的请求。

省略设置保留已有值，新账号默认 all。设置只推进配置版本，不修改
凭据或 State 绑定版本、采集计时及已保存槽位。重登、重新授权、
Token 刷新和未提供政策的导入必须保留名单；旧模板也不得重置它。
导入 settings 显式政策覆盖文档政策，未提供则采用文档或保留旧值。

## 4. 校验与错误

| 输入 | 结果 |
| --- | --- |
| `all` 和 `[]` | 明确取消限制 |
| `allowlist` / `denylist` 和 1–256 个 ID | 接受，精确匹配并去重 |
| 空名单限制、通配符、控制字符、保留前缀、超长 ID | 拒绝 |
| 设置省略或 null | 保留；不是取消限制 |
| 模型限制被拒绝 | 不获取租约，不发送请求 |
| 关联分组更新失败 | 包括模型政策在内整笔事务回滚 |

## 5. 示例

正常：只发送 `{accountIds, modelAccess}`，并发、State 和代理不变。
默认：旧库升级后 all，未显式配置的用户行为不变。
错误：把 allowlist 写进凭据，导致登录覆盖；或允许粘性账号绕过名单。

## 6. 验证

- Core：精确校验、映射别名、分组快照及整个授权池目录。
- OpenAI：普通/等待选择、软粘性、固定账号/重试与 State 组合。
- Store：真实 PostgreSQL 迁移、model-only 补丁、原子回滚和续期保留。
- Frontend：只提交勾选字段、编辑基线、不共享响应式数组、关闭清理、
  目录失败可保留/手填模型、桌面和窄屏布局。
- 老迁移测试用老结构 SQL 写入夹具，不得用新 Repository 访问不存在的新列。

## 7. 错误与正确做法

错误：只在普通选号处筛选，容量等待结束后可选到禁止账号。
正确：共享同一 scope 判断，两个路径均在 State、额度和评分前调用。

错误：分组编译时重新构造 `RuntimeAccount` 却丢失政策。
正确：保留 `.with_model_access(account.model_access().clone())`。
