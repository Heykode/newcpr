# 原生多平台合同

适用内置OpenAI、xAI与Antigravity；共享合并规则见`docs/upstream-integration-checklist.md`。
插件产品、上游API Key账号与主动互联转发不属于已接入能力。

## 平台与请求

- Antigravity使用Google OAuth/PKCE，凭据提交仍使用本地账号身份、revision及Store事务；真实目录决定模型可用性，价格表不创建可用模型。
- GenAI模型列表、详情、generateContent、streamGenerateContent与countTokens使用原生权限与协议；Chat 使用原始协议参与路由，选中 AGY 后直接转换为 GenAI。AGY 拒绝原生 Responses 和 Messages。生成和计数都先放平台提示词，随后保留客户端系统指令、工具说明和元数据。搜索工具遵循原生 GenAI/Chat 的转换合同。
- 普通基础模型组请求先按思考档位解析实际型号，再核对账号/分组权限；显式型号保留。不支持的档位不能偷偷选择别的模型。例外仅为原生非流式独立搜索：唯一工具为 googleSearch/google_search 对象且未提供 thinkingConfig，可用同账号的任一获准具体成员证明分组资格，路由到目录声明且同账号/分组允许的搜索目标。混合工具、流式及显式 thinkingConfig 不走该例外；Provider 使用 Core 的 dedicated_web_search 标记，不根据工具重复猜测执行类型。
- 原始JSON校验后交付原始字节；SSE保留原始帧、独立usage和EOF。原生GenAI首帧前及帧间不插入网关SSE心跳，首帧前失败保留HTTP错误；其他协议保活与取消/结算仍由统一交付owner管理。
- GenAI 的 x-goog-api-key 只用于网关 Client Key 鉴权，不发往上游；诊断导出在纯文本、JSON 对象、头部二元组及 name/value 对象中均须脱敏，不能只更新文本正则。
- 管理固定账号探针可检查目录未收录型号；普通请求必须遵守账号目录和人工模型限制，两者不能共用绕过。

## 身份、隐私和调度

- 三平台后台操作读取全局画像，同轮冻结；下游Key覆盖仅用于对应模型请求。OpenAI解析器与前端选项共享recognizedUserAgentPrefixes；未知UA需配套originator/Core version。
- 保留持久设备、安装种子、账号出口、Excel/State和精确WS owner。OpenAI使用Redis等待；xAI/Antigravity使用进程内等待加Store原子准入。
- 三平台共享strict/preferred/relaxed及TTL；临时分流不抢占主绑定，原生续接优先。只有从未绑定的WS不发失效提示；已绑定owner释放须通知空闲下游。
- 原生隐私四步骤为project/user/git remote/commit metadata。编译结果冻结到请求快照，在HTTP、WS及Excel最终出站执行；禁用不执行。按账号/Key/会话隔离，换号/会话轮换别名，不保留已删除组合动作。

## 设置、价目与只读共享

- settings_revision与全局config_revision分开；设置草稿CAS只受设置修改影响，事务与审计一起成功或回滚。
- 原生价目按Provider支持档位校验，默认/同步/覆盖有明确优先级；在请求计划冻结，未知价格不伪造金额。账单图片单价和倍率快照兼容旧数据；本地费用不是上游额度扣减证据。
- OAuth池只读共享必须由下游Key显式授权，默认关闭，继续受分组、模型和各共享项约束。不得暴露账号凭据或以共享权限启用主动转发。
- Antigravity近七日统计按部署时区当天及前六日；空用量保留窗口类型，与上游模型分组额度独立。
- Provider能力由后端声明；不支持的导出必须在读凭据前拒绝，未知平台身份覆盖包括显式null也拒绝。

## 验证

按固定官方提交维护文件差异账，复用有效范围内既有测试。隔离PG/Redis、真实loopback传输及浏览器分别记录；模拟协议通过不等于真实Google授权或付费上游验证。

## 模型组、搜索和额度活动

- 客户端菜单必须从 ExecutionService 的协议过滤目录取得授权画像；Codex 只显示 Responses 可执行模型，Anthropic 只显示 Messages 可执行模型。GenAI 的基础模型与 agent 菜单分别按真实成员及顺序投影。映射覆盖同名模型后不能保留旧画像。
- Chat 直接转 GenAI，使用公共 Chat 转换层；原生 GenAI 保留自身参数合同。AGY 原生 Responses/Messages 转换已随本轮批准的官方协议边界退役。
- 原生 GenAI/Chat 的搜索按目标协议工具、同账号及授权范围执行；已退役的 Responses 独立搜索转换不能作为当前可用入口。上游完整 JSON 转下游 SSE 不等于逐 Token 流。
- AGY 额度 worker 批读快照版本。健康快照满五分钟且之后有成功生成完成才补查。缺少匹配快照或耗尽到期时独立复查。同一身份仅续期令牌时，PostgreSQL 保留快照并沿用原观测时间。
- AGY 主动额度查询写回时，Store 在同一事务核对账号仍启用、凭据版本和有效代理 URL 与开始时相符。发生冲突时保留已有额度、套餐和目录。
- 同次目录确认额度组完整成员后保存bucket_models；未知保持未知，不从缺失数据推测0。

## xAI 旧会话兼容

- 旧版持久会话只有 account_id、session_id、transcript；新增 response_stored 缺失时按 false 读取，通过已有历史重放路径续接，不能假定上游保存了响应。显式 true/false 保留，非法类型仍拒绝。
- 旧记录没有 instructions 不构成解析失败；此兼容只证明本地状态可读取，不代表被官方移除的工具转换、压缩或上游密文恢复仍可用。
- 回归同时验证旧载荷解析、历史可重放及新字段的显式值。隔离解析测试不代替 HTTP/Provider 完整续接测试。

## OpenAI 在线设置与升级

- openaiRequestSettings 保存国家策略、OAuth Guardian 开关、residency（默认 us）和 accountRoutingOverride。null 显式清空；请求和后台任务冻结当轮保存值，HTTP/WS/后台 OAuth 都应用。
- openaiWebsocketPool 保存 enabled、maxAgeMs、fastPathBudgetMs、idleTimeoutMs、streamIdleTimeoutMs。流式 idle=0 使用传输默认；设置更新不重建共享池，连接寿命在创建时冻结。
- 本地每账号有效并发只限制同时 opening；成功、失败、取消释放。存活 busy/idle/closing 不占 opening；不新增抢先回收不匹配 idle 的行为，物理出口容量继续生效。普通请求并发独立且不变。
- 首次升级只导入一次旧 YAML；旧数据库寿命/流式超时覆盖优先。之后数据库为权威，Core 初始化必须在首个请求前发布池设置，不能等后台刷新才生效。
- AGY observed_plan 进入 AttemptRecord 和 PostgreSQL usage/ops 历史。None 代表未观测，显式未知不借用当前账号套餐；新旧历史不可用当前可变显示名重写。
