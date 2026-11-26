# v3.55.2 原生接入与验证记录

## 基线与范围

- NewCPR 基线：`1aab1949b38a93b145bc835734af85e2affc5528`（v3.54.5）。
- 上次官方基线：`21622eeacdab919013408a4b77f5536ed8888f32`。
- 本次官方固定目标：`20d04ae57abf61ffc7c176eb73730577aaf1b0af`（v3.55.2，包含版本发布后的表格列宽修复）。
- 遵循[共享合并规则](upstream-integration-checklist.md)。[逐文件差异账](upstream-v3552-file-ledger.json)覆盖两端的全部918项：必要适配506、行为移植88、无行为增量257、按范围排除67，没有待归类项。行为移植不表示文件或整个服务字节一致。
- 用户批准的原生更新包含Antigravity、工作区隐私、模型价目及OAuth账号池只读共享。排除插件产品/专属运行时、上游API Key账号、主动互联识别与转发；混合文件中的原生公共行为单独接入。
- 本记录验证源码实施，不代表已经提交、推送、合入main、发布或部署。真实上游验证边界见文末。

## 执行表及真实接入

下表与逐文件差异账共同记录官方来源、本地调用路径和验证。所有编译、测试、格式化及浏览器检查均在用户指定的OVH隔离环境进行。

| 编号 | 已实施行为与本地落点 | 验证及适配说明 |
| --- | --- | --- |
| E01 | 固定上述三端并逐文件审阅；本次22个官方提交的混合改动按原生/排除部分拆分 | 918项均有处置和理由，目录移动不直接视为无行为变化 |
| E02 | `apps/gateway/src/bootstrap.rs`注册三Provider、后台服务、原生协议及价目服务 | 工作区编译和严格Clippy通过；应用启动与架构测试覆盖依赖方向及私有测试owner |
| E03 | `providers/antigravity/src/credential/`接Google OAuth/PKCE、导入导出、重授权、令牌刷新；Admin原生凭据入口接本地Store | AGY97项；账号revision、重复身份与旧凭据写回保护保留，未执行真实Google授权 |
| E04 | AGY目录、候选选择和thinking解析接Core路由 | 按真实目录先解析型号再查授权；显式型号、无权限变体、缓存/刷新和未知模型回归通过 |
| E05 | `gateway-api/src/openai/genai/`、`genai_response.rs`、Protocol GenAI及AGY transport接原生生成/流式/token计数和Responses/Chat/Messages兼容入口 | API全包693项及后续解压回归；原始JSON/工具签名、SSE后置usage、错误交付、头隔离、取消和计数不产生生成费用均覆盖 |
| E06 | AGY profile、系统提示词及下游Key覆盖接全局设置/预览/真实出站 | 后端AGY身份和系统提示词测试、API覆盖补遗、浏览器Key编辑通过；转换入口拒绝previous_response_id |
| E07 | AGY quota投影与Admin账号列表/详情补近七日用量 | Admin账号59项及浏览器额度分组、未知额度、近七日空用量；本地七日统计与上游额度周期独立 |
| E08 | Core计价快照、Admin价目、Host参考价、Store价格事务及前端编辑/同步入口 | PG价目3项、Host4项、Admin价目回归、两Provider billing63项；图片单价/倍率历史快照补测通过 |
| E09 | Admin `peer_link/`、API `openai/peer_link/`、Client Key权限和Store只读共享投影 | API14项、共享PG2项、Key PG13项；默认关闭，分组/模型/字段权限交集、限流并发与审计原子性覆盖 |
| E10 | `gateway-core/src/provider_ports/session_affinity.rs`及三Provider选择器接通通用亲和/TTL | 保留严格续接和临时分流主绑定；真实Redis三平台CAS、Core亲和正反例及OpenAI/xAI/AGY回归通过 |
| E11 | Core privacy配置编译/快照、OpenAI `transport/privacy/`及HTTP/WS/Excel最终出站；前端四步骤规则和预览 | Core策略冻结/禁用/回滚3项，OpenAI隐私2库+35集成；raw JSON、forced WS、Excel生图都有修复前失败/修复后通过证据 |
| E12 | Core response-control绑定状态及API真实WS闲置交付 | Core和真实WS入口修复前失败/后通过；WS interrupt13项、混合wire/canonical9项通过，未通过重放绕过owner限制 |
| E13 | OpenAI/xAI/AGY后台OAuth、令牌、额度和目录使用全局画像；模型请求仍允许Key覆盖 | OpenAI全局画像14项、xAI身份17项、AGY97项；身份解析失败保留真实错误来源；配置失败不会遗留首查预留 |
| E14 | OpenAI macOS默认身份、完整自定义UA及识别前缀与前端选项共源 | profile5库+117集成；六种预设真实解析、CLI辅助身份完整UA、已知/未知UA编辑浏览器回归 |
| E15 | Store独立settings_revision、全局画像、隐私及运行设置原子写入 | PG3项：从0100升级、旧值保留、真实CAS冲突、价目/管理Key不误伤设置草稿、审计失败回滚 |
| E16 | Admin/Store/API/前端备份配置的storageRevision、secretConfigured与省略Secret保留 | 真实PG导出与两轮恢复通过；API不回显Secret，浏览器验证保存/测试/计划启用和换密钥后暂停 |
| E17 | 三平台导出字段及Provider能力声明；Admin读凭据前检查能力，前端确认时复核全量选中账号 | 导出/PAT回归、Admin账号和Key补遗、前端跨页缓存及打开后能力变化回归；保留本地重登材料 |
| E18 | 使用日志优先上游真实耗时，缺失时显示带≈的总耗时估算；图片费用展示 | 前端速率/账单回归；不回写上游耗时，不把估算混入性能分位数 |
| E19 | 三平台选择器、分组/Key分区、额度国际化、Dashboard画像、表格列宽及基础控件label关联 | 前端1193项、类型/构建/ESLint；1440/390/320px真实浏览器，保留Codex15套餐、多标签、Fast/Ultrafast和Excel设置；四选项宽度专项见下表 |
| E20 | 仅追加0101/0102迁移、冻结摘要及`deploy/upgrades/v3.55.2.json` | 旧迁移字节不变；旧设置/亲和/TTL保留，旧Key共享默认关闭；候选迁移树与升级计划一致 |
| E21 | 对调用链、错误出口、生产选择的后端及现行规范完成补遗 | Clippy全工作区/全部target/全部feature通过；应用架构14项、部署工具、隐私候选树和浏览器检查分别留证 |
| E22 | `release/version.yaml`、版本说明、API、架构及Trellis现行规范同步 | published为空；提交、推送、合main、标签、部署及其准确历史/镜像检查留待相应授权 |

## 补遗缺陷与失败前后证据

原始日志留在仓库外。以下只记录测试名称和日志索引，不包含真实账号、服务器或流量。

| 发现 | 依据与实际改动 | 证据 |
| --- | --- | --- |
| raw JSON/forced WS/Excel生图绕过新隐私步骤 | 官方最终出站规则适用于这些入口；补进各真实发送路径 | `native-privacy-before-02.log`、`native-excel-privacy-before-02.log`失败，`native-privacy-after-03.log`通过 |
| 配置禁用、策略复用和旧请求快照冻结未完整接入 | Core快照中复用相同编译策略，禁用不出站，非法规则仍拒绝 | `native-core-privacy-before-01.log`失败，`native-core-privacy-after-01.log`3项通过 |
| 固定账号探针被普通目录限制误拒绝 | 按固定官方模型路由区分管理探针与普通请求 | `native-core-additions-02.log`失败，`native-core-additions-after-03.log`583项通过 |
| AGY下游Key画像被API硬编码平台名单拒绝 | 改为注册Provider能力验证，未知平台含显式null仍拒绝 | `native-api-profile-override-01.log`失败，`native-api-profile-after-01.log`和`native-admin-profile-clear-01.log`通过 |
| GenAI解压错误丢失具体来源 | 保留真实解码错误映射和执行前拒绝边界 | `native-genai-decode-before-02.log`失败，`native-genai-decode-after-01.log`通过 |
| xAI后台身份错误来源丢失 | 新增Profile错误分支、静态公开提示并保留source | `native-xai-admin-source-before-01.log`失败，`native-xai-admin-source-after-01.log`通过 |
| OpenAI已知UA前缀未返回前端 | 身份解析器与selection_options共享前缀数组 | `native-openai-options-before-01.log`失败，`native-openai-profile-after-02.log`通过；浏览器已知UA无伴随字段、未知UA必须补齐 |
| 新增第四个平台后筛选框宽度不足 | 本地账号筛选布局由旧三选项宽度调整为四选项宽度，基础组件仍保留键盘和滚动能力 | `browser-platform-width-before-01.log`失败；`browser-platform-width-after-01.log`在1440/390/320px通过，同一脚本检查全部四项和窄屏布局 |

## 保留差异及运行影响

1. NewCPR保留本地账号身份、revision、凭据版本、持久设备、安装种子、出口和多标签；官方原生规则通过现有Store及事务接入，未导入其他仓库历史。
2. OpenAI使用现有Redis owner、容量等待与跨进程原子准入；xAI/AGY按官方进程内等待配合本地Store原子认领。二者运行后端不同，不能用进程内测试代替OpenAI的Redis证据。
3. Excel/State继续执行既有模型来源、连接归属、借号和上下文合同。新身份、隐私和错误交付明确接到这些入口，保留本地跨Key/跨账号隔离；不声称与官方单一部署形态完全等价。
4. 官方旧基线已有的xAI独立订阅查询，本地此前即未接入；本次仍由billing投影订阅信息。官方旧基线已有的OpenAI OAuth transport编辑控件，本地仍采用既有自动HTTP/WS及Excel/owner合同。本轮目录重组没有移除本地已有功能，二者记录为遗留差异而非本版新增功能或已确认缺陷。
5. 原生价目影响本地估算与账单快照，不能据此推断上游套餐实际扣额。此次官方两端Fast策略/内置价格没有对应变更，未宣称解决Team免费Fast或另案New API错误。
6. 自定义仓库发行仍走既有CI和验证镜像部署流程，不复制官方插件发行通道。新增迁移后失败需人工向前恢复；IPv6禁用策略、受保护服务和部署授权边界保留。

## 验证证据与边界

- 工作区全target编译、`cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`及`cargo fmt --all --check`通过。
- API693项，Core583项，AGY97项，xAI400项，OpenAI343库+1324集成是相应主体代码的完整测试结果；后续补遗分别追加身份、隐私、计费、能力和错误映射回归，不把重复测试相加成唯一总数。
- Store库12项及集成719项原先通过，唯一备份环境失败在安装匹配的PG18工具后完成真实导出和两轮恢复补测；未跳过或删除该测试。价目3项、设置3项、只读共享2项、Key13项使用真实隔离PG；owner28项、loopback出站7项、Excel/PAT与真实Redis亲和验证分别执行。
- 应用启动及其他回归60项已有通过证据，最终架构14项补齐原先的目录组织失败。Provider计费测试搬到各自私有owner后再次执行63项通过，未扩大生产API或减少断言。
- 前端最终全量1193项通过，无失败/忽略；类型、Vite构建和ESLint通过。最后的文案、示例路径及筛选宽度微调另有定向检查和浏览器证据。
- 部署/发行工具77项、部署脚本51项、隐私工具52项通过。CI规划测试更新真实新增AGY依赖关系，不削减受影响包范围。
- 候选源码树隐私扫描通过；公开OAuth客户端注册参数、原有模拟JWT、地址分类和路径正则均人工核对来源，精确记录文件和行哈希，历史记录保留。检查器、通用policy和hooks没有放宽；真实提交与完整待发布历史须在发布阶段按准确对象复查。
- 浏览器使用合成API和隔离前端，覆盖AGY OAuth/文件导入、Key画像/共享依赖、备份版本保护、额度分组/未知额度、15套餐、Fast/Ultrafast与Excel。没有使用生产账号，没有发付费模型请求。
- Host已有1项忽略的Mihomo官方内核下载/转发检查仍未执行（要求独立Linux网络命名空间）。未做真实Google OAuth、真实Antigravity上游推理、长时间生产压测或GitHub发布镜像验收；模拟协议、真实数据库和浏览器通过不构成上述结果的替代证明。
