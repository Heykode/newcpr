# 原生模型目录合同

## 来源与调用路径

固定参考为官方 v3.48.0 `975b73b33b6d247fbe08a9a23e88066467e71d7a`。
`GET /v1/models?client_version=...` → `ExecutionService::client_model_catalog` →
`CodexCredentialCatalogService::client_model_catalog`，只读已发布快照和本地账号事实。
后台 `restore` / `refresh_catalogs` 负责持久目录恢复、发现与发布。
本合同替代旧的三个账号取样、32 个客户端版本缓存及未知套餐豁免。

## 账号证据与发布

- 原生 scope 为账号 ID、credential revision、规范化套餐。相同套餐不能共享权限结论；
  套餐未知但账号自身成功目录存在时，同样以它作为资格证据。
- PostgreSQL `provider_account_model_catalogs` 保存原始文档、模型 ID、hash、观察时间和 ETag；
  保存和恢复检查当前账号 revision/套餐，晚到旧写入不能复活旧身份。
- 启动先恢复成功落库目录，不等待整个账号池联网。相同正文只解码/共享一份；
  单 Provider 错误不伪造目录，成功空目录仍撤回旧能力。
- 后台刷新并发 32，启动间隔 10ms；首个成功即可发布，后续每 128 条批量发布。
  失效通知与发布 revision 协调，旧批次不能覆盖新账号变更。辅助请求每次 15 秒。
- 客户端读取不触发网络，不按客户端版本另建远端缓存。原生版本参数仍校验
  64 字节及 ASCII 字母、数字、`.-+`。Redis ID 缓存 TTL 五分钟；30 分钟只决定刷新时机，
  不是调度证据有效期。
- 聚合仅限当前 Key 冻结授权范围及当前启用/身份状态，按模型分别选择合法来源。
  代表账号失效时扩大本地资格读取，不能漏掉同文档的其他合法候选。
  别名只改 slug；未知字段、null、对象与来源内顺序保留；响应 `private, no-store`。
- 即时及等待选号使用当前账号、凭据版本、套餐对应的最近成功目录；缺少映射后的模型时排除账号。
  目录超过刷新间隔、ETag 通知或刷新失败都不撤销排除；只有没有同 scope 的目录才交由上游验证，
  不在选号时联网。达到 128 个遗漏时使用 Store 下推查询，SQL 同样没有目录年龄截止。
- ETag 变化只合并触发后台刷新；失败保留旧成功证据及待刷新 ETag，由既有 Host
  daemon 监督和退避，不修改账号健康、身份或会话。只有该账号新观测确认了 ETag 才清理通知，
  发布其他账号不能清除它；新成功目录新增模型后恢复可选。

## 本地必要接入

- 每轮目录刷新、单账号刷新及管理员导出从 runtime policy 读取当前全局客户端画像并冻结整套身份；
  使用选中账号凭据、代理、CA 和 IPv6，活动 IPv6 失败不能偷偷走 IPv4。
  目录请求保留 `version`、`originator` 和匹配的 `client_version`，不发送安装标识头。
  网络/客户端画像不成为模型权限缓存维度。
- 本地配置源为 `provider_outbound_user_agents.configuration_json`，只读取 configuration 模式；
  旧 default/custom/custom-auto-version 沿当前已发布画像。读取/解析失败返回错误，不回退成伪默认。
  预热复用相同全局选择；诊断从 Core 的配置快照冻结全局画像，不读取任意 Client Key 画像。
- Excel 目录使用独立 BPS 凭据查询实际 access/models，不再从旧 Excel 静态模型配置合成权限。
  scope 使用投影后的 Excel 凭据版本、套餐和 `excel-independent-v1` adapter 标识。
  Excel 成功目录只进入自己的内存/短期缓存，不写入 Codex 的持久目录事实；失败不能回退到
  Codex 凭据或旧静态模型。原生目录保留自己的账号、凭据版本和套餐权限筛选。
- Excel 多代理能力按当前授权、启用、实际 Excel 路由和模型政策，显式将对应型号的
  `multi_agent_version`、`multi_agent_reasoning_effort` 设为 null；不得扩展到范围外模型。
  查询仅重读缓存索引中的 Excel 账号；删除账号即使索引尚未刷新，也因账号不存在被排除。
- Core 继续保留已知 Provider 归属、模型映射、权限、能力及强制/排除 Provider。
  未知模型仅有唯一合格非完整目录 Provider 时可透传，多候选不能猜测。
  原生 Codex 不接受 Grok；账号入口开关冻结 Excel 路由后，只接受其实际目录与共享模型策略的交集。
- 新模型发现、上游透传和本地价格独立；不以缺少定价认定请求不支持。
- 管理端观察时间来自实际成功目录；无时间返回 null，不把读取缓存当作新观测。

## 验证

官方账号级并集、身份/套餐变化、候选替补、启动持久恢复、并发发布、万账号恢复与遗漏筛选；
本地代理/IPv6/installation、Excel 独立目录、未知套餐自身权限、排队提升前重查、管理员导出。
API 同时验证普通 OpenAI 列表、Codex 目录和显式 Anthropic 头下的分页/详情。
不把 mock 上游通过写成真实上游或生产验收。

## 8. 管理员指定账号导出

### Scope / Trigger

管理员显式下载单个账号的原生目录；不是客户端目录的后台聚合或推理路径。

### Signatures

- `GET /api/admin/accounts/models/catalog?accountId=...`。
- `AccountsService::model_catalog_document` 经所选 Provider 的同名 port，
  进入 `CodexCredentialCatalogService::account_catalog_documents`。
- 成功 envelope 的 `data` 为 `{modelCount, observedAt, catalog}`；
  `catalog` 为完整的 `{"models":[...]}`，响应 `private, no-store`。

### Contracts

只加载指定账号的凭据，允许显式指定已停用账号但不启用它。从持久化全局选择冻结
profile，保留原账号代理、IPv6、CA 和 15 秒超时。不得读取其他账号补结果、
回退套餐缓存、发布全局目录或生成模型调用。目录不发送安装标识头；读取目录不改写账号的持久设备身份。
保留原生对象未知字段、null、嵌套结构和模型数组顺序。

### Validation & Error Matrix

- 未登录：401，不调用 Provider；缺失、非法或额外 query 字段：400。
- 原生请求失败、空目录、失效出口：返回既有错误，不导出旧缓存或伪目录。
- 非 Codex 文档：拒绝；活动 IPv6 不可用：失败，不回退 IPv4。

### Good / Base / Bad Cases

Good：停用账号用自己的授权与出口导出。Base：普通模型列表行为不变。
Bad：以同套餐另一账号成功目录掩盖当前账号的 403。

### Tests Required

Provider 测试覆盖精确授权、禁用账号、原对象、失败、冻结 default/custom 画像、
代理认证和真实 IPv6 peer/负例；Admin 测试确认单账号派发且无 mutation；
API 测试实际执行认证、严格 query 和成功 no-store 路由。执行状态单独记录，
不能把仅新增回归代码视为通过。

### Wrong vs Correct

Wrong：用后台聚合目录或同套餐其他账号的缓存替代指定账号导出。
Correct：直接使用所选 `ProviderAccount` 和冻结 client 的既有辅助 GET。
