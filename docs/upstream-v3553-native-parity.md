# v3.55.3 原生接入与验证记录

## 基线与范围

- 本地起点：`60ebb0f17522ba1a15c6340260135c85eeb2710f`（上一批v3.55.2，尚未推送）。
- 官方起点：`20d04ae57abf61ffc7c176eb73730577aaf1b0af`；固定目标：`88da31137b32d11015cbe0727b8b056f4476de7e`。
- 官方v3.55.3标签为`ef45165d8d30cfaee65558807b401541503d8aea`，本次包含标签后的协议修复。
- 直接比较两端固定源码树及GitHub清单一致：9个提交、75个文件。逐文件处置见[差异账](upstream-v3553-file-ledger.json)，遵循[共享合并规则](upstream-integration-checklist.md)。
- 本轮包含Antigravity和全部适用原生增量；排除插件专属运行时、上游API Key账号、主动互联转发。混合文件中的原生规则单独接入。
- 最近部署记录仍为v3.54.5 / `1aab1949b38a93b145bc835734af85e2affc5528`，本轮未操作生产。当前是源码候选，不代表提交、推送、合main、标签或部署。

## 实际接入与验收

| 编号 | 本地实际调用链与行为 | 适配及验证 |
| --- | --- | --- |
| E01 | 75个官方变更文件逐项归类，记录真实owner和排除部分 | 无依赖或迁移增量，验证状态见下文 |
| E02 | AGY quota worker→reconcile→ProviderAccountStore轻量版本/活动查询→PG request_analytics_facts | 健康快照满五分钟且之后有成功生成才补查。缺少匹配快照和耗尽到期时独立复查。PostgreSQL 在同身份续期时沿用已有快照和原观测时间 |
| E03 | 同次目录确认bucket_models→ProviderQuota Models→AccountsService窗口查询→PG聚合→前端窗口Token | 保留本地reset/window_seconds边界；已知无用量显示0，未知不补0。Models不进入整号估值、预测、恢复或分组监控 |
| E04 | 原生GenAI路由→stream_execution_response_as→首帧等待/输出循环 | 关闭该协议首帧前与帧间保活；其他协议和取消/结算owner保留，删除不再使用的GenAI保活错误helper |
| E05 | AGY catalog/model_groups→Core snapshot→ExecutionService::client_model_catalog实际fallback | 按Key授权成员裁剪档位、默认档位、图片、上下文及描述；映射覆盖使用目标画像。真实客户端调用者必须同步，不能只移植snapshot helper |
| E06 | Operation要求→Core plan_inner→候选scope→实际Provider选号 | 按具体型号/账号输出上限排除超限者，未知上限交上游；本地exclude_accounts沿restricted_accounts求交，不扩大严格绑定 |
| E07 | 公共web_search选择解析→Responses内容/工具转换→AGY prepare | 可选搜索不排斥函数工具；强制选择缺失或歧义明确拒绝，不默默普通生成 |
| E08 | Core同账号同分组授权→AGY目录首个搜索目标→GenerateMode::WebSearch→grounding→下游终态 | 实际搜索型号进入元数据、usage和计价；支持域名过滤、来源和引用；沿既有租约释放和取消合同 |
| E09 | Responses→responses_to_genai_request→AGY生成 | 复用内容映射，参数直接投影；省略/null不补默认输出上限，推理预算不继承Messages专有约束，采样/JSON Schema保留 |
| E10 | OAuth Messages请求转换和Responses响应转换→ProviderError→GatewayError | source私留、发送状态和重试边界保留；静态公开搜索错误不包含原始输入。API Key专属反向转换不引入 |
| E11 | 共享Core/Admin/Protocol/API及原生Provider回归 | 保留账号身份、revision、出口、多标签、15 Codex套餐、Excel/State、Redis owner/队列、严格续接、Fast/Ultra Fast和OAuth池只读共享；未涉及分支复用上一批有效证据 |
| E12 | 版本说明/API/架构/现行spec及升级计划 | version=3.55.3、published={}；从v3.54.5升级计划仍含0101/0102，旧迁移字节不变 |

## 缺陷证据与必要接入

原始日志在仓库外，所有编译、测试、格式化及浏览器验证仅在用户指定OVH隔离环境。

| 项目 | 失败前证据 | 修复后验证 |
| --- | --- | --- |
| 原生GenAI心跳提前返回及帧间额外注释 | v3553-before-genai.log：10通过、2行为断言失败 | API原生GenAI合同 |
| 可选搜索与函数工具冲突 | v3553-before-optional-search-effective.log：实际运行1项并因InvalidRequest失败 | AGY optional_search_preserves_functions_and_unspecified_output_limit |
| OAuth Messages转换丢失错误source | v3553-before-translation-source.log：1项因source缺失失败 | oauth_messages_translation_preserves_private_error_source |
| 已知0 Token被隐藏 | v3553-before-quota-ui.log：null与0断言失败 | quota countdown用例及浏览器 |
| 新授权画像必须接实际客户端目录调用者 | v3553-before-catalog-caller.log：只恢复旧fallback调用后，实际返回medium/high而授权范围仅medium，1项行为失败 | v3553-after-catalog-caller.log：同一入口1项通过 |
| 上一批API测试树清单仍用旧peer_link路径 | v3553-native-tests.log：696通过、1项架构失败 | 仅更新为真实mod.rs路径，不放宽检查 |

早期optional-search日志曾运行0项，不计为有效证据；后续已强制刷新测试文件并取得上表有效失败。
目录调用者实验是在本次新画像能力下恢复旧调用，不将它描述为上一已发布版本曾泄露这项新能力。
PG首轮为117通过、1项测试夹具外键失败：官方夹具仅写账号引用，本地请求还引用账号实体。
补齐合成账号后，活动回归1项通过；未修改数据库外键或放宽业务断言。

## 运行边界

- 账号目录取得webSearchModelIds后才可独立搜索；已有账号需刷新目录。额度组成员通过后续额度刷新补齐；旧快照缺失成员时继续未知。
- 搜索仅提交最后用户文本，不发送编码历史；只支持实时联网、文本输出和允许/屏蔽域名。上游完整JSON转换下游SSE，不是上游逐Token搜索流。
- 常规AGY生成/计数替换客户端系统提示词，独立搜索使用固定搜索指令；非null previous_response_id续接仍拒绝。这是固定官方仍有的边界，并非本轮消除的限制。
- 官方插件运行时改写后的再验证在本地无运行入口；原生请求按同一解析器在选号和转换处判定。OpenAI上游API Key专属转换器未接入，OAuth和公共协议改动已接。
- 本地计费不证明上游套餐额度消耗；本轮不宣称改变Team免费Fast、Ultra Fast支持范围或解决其他生产错误。
- 从已部署v3.54.5升级仍跨0101/0102迁移，失败人工恢复；部署、切换及是否备份由后续本次部署授权决定。

## 验证状态

以下编译、测试、格式化和浏览器检查均在OVH隔离验证环境执行，CI模式禁止缺失数据库配置时静默跳过。

| 范围 | 已取得结果 | 仓库外日志 |
| --- | --- | --- |
| 工作区编译 | `cargo check --locked --workspace --all-targets`通过 | v3553-check.log |
| Admin | 9项单元、482项集成通过 | v3553-native-tests.log |
| API | 2项单元、696项集成首轮通过；纠正旧路径后5项架构检查通过，覆盖原失败项 | v3553-native-tests.log、v3553-api-architecture.log |
| Core | 7项单元、585项集成通过；实际目录caller额外取得前失败后通过 | v3553-native-remaining.log、v3553-before-catalog-caller.log、v3553-after-catalog-caller.log |
| 协议 | 170项通过，包含Responses参数直转、工具选择与grounding | v3553-native-remaining.log |
| Antigravity | 103项通过，包含活动刷新、搜索权限、实际型号计价和租约释放 | v3553-native-remaining.log |
| OpenAI | 345项单元、1365项集成通过，包含Excel、国家策略、严格续接和转换错误 | v3553-native-remaining.log |
| 真实PostgreSQL | 117项首轮通过，补齐夹具后剩余活动查询1项通过；覆盖轻量版本、模型窗口、预测排除、账号身份与revision | v3553-pg-accounts.log、v3553-pg-activity-after.log |
| 前端 | 33项quota回归、vue-tsc、ESLint、Vite构建通过 | v3553-quota-ui.log、v3553-frontend-types.log、v3553-frontend-lint.log、v3553-frontend-build.log |
| 真实浏览器 | 1440/390/320px通过；已检查桌面和最窄额度截图，已知0与未知分开显示 | v3553-browser.log及截图 |
| 升级准备 | 部署工具23项通过；100份冻结迁移摘要通过；迁移树与升级计划一致，本批未改旧迁移 | v3553-deploy-tests.log、v3553-migrations.log |
| 严格静态检查 | 全工作区、全target、全feature Clippy以`-D warnings`通过；`cargo fmt --all --check`通过 | v3553-clippy-final.log、v3553-fmt-check.log |
| 最终路由与架构 | 清理测试冗余clone后29项Core路由通过；App的14项架构检查通过，包含模块布局和依赖边界 | v3553-core-routing-final.log、v3553-app-architecture.log |

E01–E12的源码实施与隔离验收完成。共85个本地变更文件，包括75个官方文件的映射及额外接入、测试、规范和升级文件；差异账单独列出所有本地额外owner。
候选一致性按全部2467个源码文件逐字节SHA-256核对，记录于仓库外v3553-source-consistency.json；后续说明文档同步不改已验证的程序源码。
隐私检查使用独立临时index生成准确候选树，不改实际Git暂存区；最终树和扫描结果保留在仓库外。
本轮没有提交、推送、合main、打标签或部署；本地没有生成target、node_modules、dist或浏览器测试产物。
未执行真实Google OAuth、付费上游推理或生产压力测试；模拟协议、真实隔离PG和浏览器通过不能替代这些验证。
未修改Redis实现；其真实后端合同复用上一批已记录证据，本轮Core/OpenAI回归不能代替新的Redis服务端测试。

## 两批提交后的复查

复查 `60ebb0f175` 和 `8830fd30fe` 时确认一处原生行为漏接。v3.55.2 官方 AGY 主动查额度时会记录账号当时的连接信息。此前本地只核对凭据版本，查询期间换代理或停用账号都可能让旧结果写回套餐、额度并继续发布模型目录。现由 AGY 调用带连接校验的 Store 写入。PostgreSQL 在更新事务内核对启用状态、凭据版本和有效代理 URL。冲突时停止后续目录发布。OpenAI 和 xAI 继续沿用各自原有的额度写入规则。

OVH 的 `quota_refresh_discards_response_after_account_proxy_changes` 在旧实现下实际运行一项并失败，修正后通过。AGY 全包 104 项通过。PostgreSQL 账号适配器 6 项通过，其中新增用例覆盖只停代理、停用账号、冲突后无快照，以及恢复后当前连接可正常写入。全工作区所有目标和特性的严格 Clippy 通过。该验证使用模拟上游和隔离 PostgreSQL。

另核对了同身份令牌续期。官方与本地 PostgreSQL 均保留已有健康额度和原观测时间，续期本身不会强制重查。已修订 E02、API 及版本说明。强制搜索对原型号和实际搜索型号均校验已知输出上限，两者须获同一账号和分组授权。Codex 分组 Fast 策略、严格排队与续接、Excel/State、只读共享、独立设置版本和套餐筛选，沿实际入口抽查并参考此前 OVH 证据，未发现本次新增回退。
