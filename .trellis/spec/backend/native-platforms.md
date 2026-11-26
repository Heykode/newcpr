# 原生多平台合同

适用内置OpenAI、xAI与Antigravity；共享合并规则见`docs/upstream-integration-checklist.md`。
插件产品、上游API Key账号与主动互联转发不属于已接入能力。

## 平台与请求

- Antigravity使用Google OAuth/PKCE，凭据提交仍使用本地账号身份、revision及Store事务；真实目录决定模型可用性，价格表不创建可用模型。
- GenAI模型列表、详情、generateContent、streamGenerateContent与countTokens使用原生权限与协议；兼容Responses/Chat/Messages走同一Core生命周期。常规生成/计数由服务端系统提示词覆盖客户端系统指令；独立搜索使用固定搜索指令。转换入口拒绝非null previous_response_id。
- 思考档位先解析为目录中的实际型号再核对账号/分组权限；显式型号保留。不支持的档位不能偷偷选择别的模型。
- 原始JSON校验后交付原始字节；SSE保留原始帧、独立usage和EOF。原生GenAI首帧前及帧间不插入网关SSE心跳，首帧前失败保留HTTP错误；其他协议保活与取消/结算仍由统一交付owner管理。
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

- Codex适配目录必须从ExecutionService实际fallback调用授权scope画像，不能只修内部helper；推理档位、图片、上下文与描述由可用成员决定。映射覆盖同名模型后不能保留旧画像。
- Responses参数直接转GenAI：省略/null输出上限不补默认值，推理预算不继承Messages约束；内容和工具转换继续复用公共协议映射。
- 可选搜索与函数工具共存。强制搜索按同账号同授权分组同时允许原型号/目标型号选路，实际输出上限和计价使用目标型号；无能力不降级普通生成。
- 搜索仅发送最后用户文本，实时联网及域名白/黑名单按支持范围校验；来源和引用由上游grounding生成。上游完整JSON转下游SSE不等于上游逐Token流。
- AGY 额度 worker 批读快照版本。健康快照满五分钟且之后有成功生成完成才补查。缺少匹配快照或耗尽到期时独立复查。同一身份仅续期令牌时，PostgreSQL 保留快照并沿用原观测时间。
- AGY 主动额度查询写回时，Store 在同一事务核对账号仍启用、凭据版本和有效代理 URL 与开始时相符。发生冲突时保留已有额度、套餐和目录。
- 同次目录确认额度组完整成员后保存bucket_models；未知保持未知，不从缺失数据推测0。
