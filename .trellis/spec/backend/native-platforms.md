# 原生多平台合同

适用内置OpenAI、xAI与Antigravity；共享合并规则见`docs/upstream-integration-checklist.md`。
插件产品、上游API Key账号与主动互联转发不属于已接入能力。

## 平台与请求

- Antigravity使用Google OAuth/PKCE，凭据提交仍使用本地账号身份、revision及Store事务；真实目录决定模型可用性，价格表不创建可用模型。
- GenAI模型列表、详情、generateContent、streamGenerateContent与countTokens使用原生权限与协议；兼容Responses/Chat/Messages走同一Core生命周期。服务端系统提示词覆盖客户端系统指令；转换入口拒绝previous_response_id。
- 思考档位先解析为目录中的实际型号再核对账号/分组权限；显式型号保留。不支持的档位不能偷偷选择别的模型。
- 原始JSON校验后交付原始字节；SSE保留原始帧、独立usage和EOF。错误协议、取消、保活和结算仍由统一交付owner管理。
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
