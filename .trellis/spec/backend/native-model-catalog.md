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
  egress revision、client version、effective profile。
- 最多 32 项，单次 15 秒，成功 TTL 五分钟，失败从完成时起 TTL 五秒。
  同键初始化合并；失效仍保留在途所有权，取消后可回收无持有者的初始化槽。
- 账号停用/删除或出口快照不可用时，不得返回旧缓存成功。
- OpenAI 的发现型目录不能证明未列出的模型不可用。Provider 的
  `model_catalog_is_exhaustive` 返回 false，Core 仍保留映射、账号权限和 Provider 排除，
  即时/等待选择器均不得再次使用目录缺失淘汰账号。其他 Provider 默认维持完整目录准入。
- 新型号的目录发现、请求透传与本地计费是独立合同。接入型号时分别验证原生目录、
  reasoning effort、实际发送的模型单价和搜索工具费率，不以缺少价格认定请求不受支持。
  价格只录入已核验的精确 ID，不扩大账号白名单或受管 State 型号，不改写旧型号别名。
- Responses 观测到新模型 ETag 时只合并触发一份后台刷新；失败保留最近成功目录并按
  1、2、4、8、16、32、60 秒有界退避，后续封顶 60 秒。成功重置退避；关闭可立即
  取消等待。目录辅助请求失败不修改正常推理账号的调度、身份或会话状态。

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
| OpenAI 推理或显式 compact 的模型未被发现 | 保留账号权限和调度约束，交由上游判断 |

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

## 7. Wrong vs Correct

Wrong：照搬官方裸 HTTP client，保留业务模型字段却丢失本地账号出口。

Correct：从缓存键的冻结画像构建既有账号 client，经原出站控制面获取连接，
并用实际 HTTP 请求验证头部与源地址，而不是只比较配置对象。
