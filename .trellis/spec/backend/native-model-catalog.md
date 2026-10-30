# 原生模型目录合同

## 1. Scope / Trigger

修改客户端模型目录、原生能力字段、别名、目录缓存或辅助 HTTP 出站时适用。
固定参考为官方 `b6e17db0`，本地还必须保留账号设备与出口合同。

## 2. Signatures

- `GET /v1/models?client_version=...`：认证后进入原生目录路径。
- `ExecutionService::client_model_catalog`：冻结客户端权限，不启动计费执行。
- `ProviderModelDescriptor` / `PublicModelDescriptor`：Core 只解释模型标识，
  原生对象通过 `RawJsonPayload` 保存。
- `CodexCredentialCatalogService::client_model_catalog`：在冻结账号范围内取样。
- `RuntimeSnapshotCompiler::new` 使用 `Arc<dyn ProviderCatalogPort>`。

## 3. Contracts

- 无非空客户端版本时保留普通 OpenAI 模型列表；原生目录只在查看目录时读取，
  不给每次推理增加目录调用。
- 版本最多 64 字节，仅 ASCII 字母、数字和 `.-+`；无权限账号不可成为候选。
- 稳定排序并最多尝试三个可用账号；原生失败不使用跨账号范围或套餐缓存
  拼出成功结果。别名只改 `slug`，原始未知字段、null、嵌套对象及顺序保留。
- 原生结果仍过滤当前不可路由模型，响应为 `private, no-store`，
  不借用上游 ETag 表示改写后的正文。
- 冻结 effective profile 并使用同一份画像发出 HTTP 请求；保留账号代理、
  自定义 UA、CA、IPv6 及失败类型。活动 IPv6 不得偷偷退回 IPv4。
- 账号上下文使用自己的 installation ID；是否上网发送由该端点已有协议决定，
  不能为了“对齐”向模型目录臆造 installation 头。
- 缓存键包括账号、credential revision、上游用户/账号、套餐、代理、
  egress revision、client version、effective profile，以及 Excel 模式/模型列表。
  后两项仅隔离模型目录缓存，不改变推理缓存键或会话编号。
- Excel 多代理密文能力例外按 Sub2API #118：当前冻结授权范围内，持久启用的 OAuth
  Excel 账号对其配置且模型政策允许的型号，将 `multi_agent_version` 与
  `multi_agent_reasoning_effort` 显式限制为 null。限制在原始目录缓存之后施加，
  不因临时凭据/额度失败恢复 v2；其他原始字段、范围外和纯原生型号不变。
  别名仍由 Core 选择目标对象，不修改选号或请求路径。
- 最多 32 项，单次 15 秒，成功 TTL 五分钟，失败从完成时起 TTL 五秒。
  同键初始化合并；失效仍保留在途所有权，取消后可回收无持有者的初始化槽。
- 官方 `aa0ce5203`：不再以 1 MiB 拒绝目录正文；驻留原生目录正文总预算为 32 MiB。
  超大单次结果仍可完整返回，但移出缓存；超预算时逐出已完成项，不取消在途请求。
- 账号停用/删除或出口快照不可用时，不得返回旧缓存成功。
- OpenAI 的发现型目录不能证明未列出的模型不可用。Provider 的
  `model_catalog_is_exhaustive` 返回 false，Core 仍保留映射、账号权限和 Provider 排除，
  即时/等待选择器均不得再次使用目录缺失淘汰账号。其他 Provider 默认维持完整目录准入。
- 官方 `2f96c02e` 的模型归属先于账号范围、能力、强制/排除 Provider 判定：
  已知模型只允许声明支持它的 Provider；声明者不可用不能变成其他 Provider 的未知透传。
  多个 Provider 共同声明的模型保留原候选顺序。未知模型只允许一个有账号模型权限的
  非完整目录 Provider 透传，多候选且未明确指定时拒绝歧义；强制 Provider 不能扩大权限。
- Codex OAuth 即时/等待选择器都拒绝 `grok` / `grok-*`（含路径前缀及大小写变体），
  不以目录缺失拒绝其他未知型号。按请求模型解析为 Excel 的路径保留原资格合同。
  管理员固定账号诊断保留原有本地模型政策豁免；质量检测仅豁免其指定账号，
  两者都不能绕过已知 Provider 归属。
- 后台实时编译时，单 Provider 查询 Err 只复用该 Provider 上次成功的模型、能力、
  展示、目录完整性和目录账号事实；成功空列表必须撤回模型。账号、权限及容量
  始终从新配置构建，停用检查继续由现有即时资格校验执行；旧目录账号不得复活
  或扩大 `/models` 可见范围。
  冷启动失败不伪造旧目录；实时刷新仍校验目录 generation；配置提交继续使用不查询
  远端的缓存目录路径，并保留优先发布、取消和后续目录对账合同。
- 新型号的目录发现、请求透传与本地计费是独立合同。接入型号时分别验证原生目录、
  reasoning effort、实际发送的模型单价和搜索工具费率，不以缺少价格认定请求不受支持。
  价格只录入已核验的精确 ID，不扩大账号白名单或受管 State 型号，不改写旧型号别名。
- Responses 观测到新模型 ETag 时只合并触发一份后台刷新；失败保留最近成功目录与待刷新 ETag，
  worker 返回带 cause 的错误，由 Host 记录健康状态并按 1、2、4、8、16、32、60 秒
  有界退避重启，后续封顶 60 秒；不得在 daemon 内吞错重试。成功才报告 succeeded，
  由 Host 重置退避；关闭可立即取消等待和刷新。目录辅助请求失败不修改正常推理账号的调度、身份或会话状态。
- 管理端账号模型查询和强制刷新同时返回 Provider 最近成功获取目录的 observedAt，
  observedAtDisplay 按部署时区格式化。无观察时间返回 null / —，前端不展示伪造时间。
  模型目录仅表示模型 ID 被发现，不表示所有端点已验证；不改变路由或模型政策。

## 4. Validation & Error Matrix

| 输入/状态 | 结果 |
| --- | --- |
| 缺少或无效客户端认证 | 不调用目录 |
| 超长、空白混入或非法版本 | 400，不查询上游 |
| 账号范围为空、无可用账号、上游目录错误 | 503，不补伪目录 |
| 同账号同版本同画像并发请求 | 合并上游初始化 |
| 重新授权、代理/出口或画像变化 | 使用不同缓存键 |
| 在途条目失效且容量已满 | 保持容量约束，不遗忘旧在途调用 |
| 普通模型列表 | 继续返回发现结果，不补造模型 |
| 模型仅未被 OpenAI 发现，且全局无已知声明者 | 保留账号权限和调度约束，唯一合格 Provider 可交由上游判断 |
| 模型已由其他 Provider 声明，该 Provider 被排除或无权使用 | 不回退 OpenAI 透传 |
| 冷启动多个目录失败，未知模型存在多个候选 | 拒绝歧义；合法强制 Provider 可继续 |
| 单 Provider 刷新报错，同时删除账号或撤销权限 | 保留模型资料，立即采用新账号与权限事实 |
| 成功获取空目录 | 撤回该 Provider 原有模型资料 |

## 5. Good / Base / Bad Cases

- Good：自定义 UA 账号经自己的 IPv6 请求新版本目录，返回完整模型对象。
- Base：普通 `/v1/models` 继续返回 `object=list`。
- Bad：用另一个套餐的提示词和能力补齐当前账号失败的目录。
- Bad：缓存中有成功数据就跳过当前账号和出站状态校验。

## 6. Tests Required

- API 版本矩阵、完整未知字段/别名、权限过滤、no-store 和失败合同。
- 真实 loopback HTTP 代理认证、IPv6 peer 源、default/custom 画像一致性。
- 凭据/身份/套餐/版本/出口独立键、并发合并、失效容量与取消回收。
- 测试保持与生产相同的叶子 `tests/credential/catalog.rs`；新增
  `native_catalog_local.rs` 场景由 `tests/credential/mod.rs` 正常注册，
  仅在测试模块之间共享夹具，不用 `#[path]` 或改变生产目录来迁就测试。
- 组合回归必须包含 Store snapshot 构造、Core、API、Provider 和 App
  架构门禁，不以单个 Provider crate 通过代替组合通过。
- 发现目录已成功返回但缺少新模型时，验证普通、映射、显式 compact 和排队请求仍到达上游；
  同时验证被排除 Provider、账号范围、健康及并发约束没有放宽。
- 验证已知归属的映射、共享声明、能力/权限/强制 Provider 负例；冷启动失败时的
  未知模型单候选、多候选及授权过滤；原生 Grok 在 HTTP/WS、等待开关两种选择器
  均不得取得租约或发送请求，同时保留 Excel 准备和其他未知原生模型透传。
- 目录失败回退需覆盖成功空目录、模型展示/能力/完整性/目录账号、独立 Provider 更新、
  删除账号与新权限/容量、generation 变化重试及已有配置发布抢占回归。

## 7. Wrong vs Correct

Wrong：照搬官方裸 HTTP client，保留业务模型字段却丢失本地账号出口。

Correct：从缓存键的冻结画像构建既有账号 client，经原出站控制面获取连接，
并用实际 HTTP 请求验证头部与源地址，而不是只比较配置对象。

## 8. 管理员指定账号导出

### Scope / Trigger

管理员显式下载单个账号的原生目录；不是客户端目录的候选取样或推理路径。

### Signatures

- `GET /api/admin/accounts/models/catalog?accountId=...`。
- `AccountsService::model_catalog_document` 经所选 Provider 的同名 port，
  进入 `CodexCredentialCatalogService::account_catalog_documents`。
- 成功 envelope 的 `data` 为 `{modelCount, observedAt, catalog}`；
  `catalog` 为完整的 `{"models":[...]}`，响应 `private, no-store`。

### Contracts

只加载指定账号的凭据，允许显式指定已停用账号但不启用它。冻结当前 effective
profile，保留原账号代理、IPv6、CA 和 15 秒超时。不得读取其他账号补结果、
回退套餐缓存、发布全局目录、生成模型调用或添加 installation 头。
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

Wrong：调用 `refresh_account_catalog`，其内部可能按套餐取样其他账号。
Correct：直接使用所选 `ProviderAccount` 和冻结 client 的既有辅助 GET。
