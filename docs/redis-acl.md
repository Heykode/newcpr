# Redis ACL 部署与回归

CPR 支持 Redis 命名用户认证，不负责创建 Redis 用户或修改服务端 ACL。

使用外部 Redis 时，在 `store.redis.url` 中填写用户名，例如
`redis://cpr_acl@redis.example.invalid:6379/0`；密码仍单独设置为
`store.redis.password`，遵循现有的 48 位十六进制校验。
不要把密码写进配置 URL。`CPR_REDIS_URL` 会覆盖配置文件 URL，
因此设置环境变量时也必须保留用户名；密码可由 `CPR_REDIS_PASSWORD` 覆盖。

自带 Compose 的 Redis 默认使用密码认证。外接命名用户时，需要同步调整
连接地址和 Redis 服务端的账号权限，不能只修改 CPR 用户名。
修改生产 ACL 前应备份，并保留可恢复的管理员连接；不要照搬测试中的
关闭默认用户操作。

## 隔离验证

CI 先运行现有密码认证回归，再在本次作业的临时 Redis 容器创建
`cpr_acl` 命名用户，关闭默认用户，使用命名用户重跑 Store 测试。
测试环境授予 `~* &* +@all` 以覆盖完整 Store 命令面，这不是生产最小权限建议。
测试覆盖连接、Lua 脚本、租约、容量等待、取消清理、发布订阅和缓存。
缺少 `CPR_TEST_DATABASE_URL` 或 `CPR_TEST_REDIS_URL` 时，CI 会失败，
本地跳过不能记为集成验证通过。
