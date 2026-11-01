# 出站 UA 选择对齐

基准为官方 `fbdfe22757cc8a4057cf598005fdac58e0ca5a18`，承接设置调整提交
`11639955c`。本次不修改生产配置，不部署，也不修改等待或重试预算。

## 行为与适配

| 范围 | 实现 |
| --- | --- |
| 旧 default/custom/custom-auto-version | 保留原有读写和运行语义，不自动改写已保存 UA |
| 新官方预设 | Desktop/CLI、macOS/Linux/Windows、架构、CLI Core/TUI/Exec、Latest/Fixed |
| 版本联动 | 按平台核验版本；Desktop Core、应用版本和构建号配套；CLI 入口后缀使用同一 Core 版本 |
| 自动更新 | 官方 npm 元数据及 Desktop 制品核验；失败保留上次版本，禁止降级发布；可选择滞后 1 至 10 个已观察版本 |
| 精确自定义 | 官方 identity 解析器，单行 ASCII 最多 4096 字节；未知产品必须提供配套头，冲突拒绝保存 |
| 旧输入能力 | 现有系统版本、客户端版本号、自动同步入口保留；新预设同样支持系统版本和固定客户端版本 |
| 请求一致性 | 在原有请求快照锁中冻结配置与版本资料；重试继续使用同一快照 |
| 身份边界 | 不改设备、会话、TLS、调度、出口或缓存键算法；显式保存新 UA 后才改变所选客户端身份 |

官方 selection、identity、CLI 发布核验、Windows MSIX/Linux DEB 制品解析及对应
测试移入原生 Provider。适配点是本项目的 raw_user_agent 表示、Provider 缓存键、
独立 Desktop 辅助请求画像、已有请求快照和管理端端口，不直接替换这些原生合同。
新版结构化配置使用独立 configuration 模式；旧自动模式仍按旧规则运行，不能将其
描述为自动迁移到平台选择模式。版本滞后基于已观察的历史，不保证刚启动就存在十个版本。

## 保存与恢复

迁移 0082 只新增可空配置文档并扩展约束，旧行的模式、字符串和时间戳保持不变。
新模式启动先恢复发布缓存，再解析已保存选择。管理端预览和保存复用 Provider 解析器；
非法配置不会替换已生效画像。UA 变化纳入质量检测和 Excel 恢复操作的配置作用域。
业务备份使用整表 pg_dump，包含新列。旧二进制不识别新模式，不支持无准备回退。

## 重试与等待说明

本次未新增重试数据库列。已有 request_tuning_json 中 websocketMaxRetries 是 Web
持久化覆盖值；Provider 用它初始化同一个 stream_max_retries 预算。没有 Web 覆盖通道
时才使用启动值，二者不累加。官方当前启动装配读取 stream_max_retries，不提供该字段
Web 热修改；这是实现方式，并非该参数在技术上不能热更新。

官方 maxWaitingPerAccount 和 concurrencyWaitTimeoutSeconds 分别是每账号等待人数
及等待时长，两条等待路径共用，新安装默认 10 人/30 秒；Key 队列也共用此时限，
Key 等待人数另有独立字段。严格续接仍保留官方 follow_only 特殊边界，并非因此允许换号。
后续账号等待对齐将原账号与全忙候选等待合为 maxWaitingPerAccount 和
accountConcurrencyWaitTimeoutSeconds 两项，两条路径共用，默认 10 个请求/30 秒。
迁移 0083 按用户要求退休旧四项并采用 10/30，已存在的新两项不被覆盖。
Key 等待配置仍独立；本轮只统一账号等待，不扩大到 Key 排队。

## 验证

全部运行验证在 OVH 隔离环境执行，使用合成账号、独立 PostgreSQL/Redis 和浏览器
模拟接口。关注旧模式升级保留、结构化配置往返、失败不污染、请求快照、发布解析、
前端编辑/预览/保存/重新加载及不同视口。测试结果记录于任务验收记录。
