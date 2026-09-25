# Excel 功能停用与移除

## 功能标识

- 稳定标识：`excel-upstream`；提交使用 `feat(excel)` 前缀。
- 引入分支：`codex/excel-upstream-20260925`。
- 引入前基线：`99400d79d0513e3a4653ffc18442847426165ae0`。
- 协议、配置和已知限制见 [Excel 上游](./excel-upstream.md)。
- 本文是未来移除的操作边界，不是已执行的删除或部署记录。

本次同时接入 Excel 和退役旧托管 State。未来只移除 Excel 时，目标是
保留当前 CPR 的原生 Codex 请求链路，而不是恢复旧 State 采集和注入。
**不要直接回退整个功能提交、合并提交或引入前的整棵源码。**
后续共享修复也不能因移除 Excel 而丢失。

## 先停用，不必立即删代码

1. 在账号管理中将已启用账号的 `responsesUpstream` 改回 `codex`。
   可使用批量编辑；没有单独的 Excel 全局总开关。
2. 核对账号模板和自动导入来源，不再显式提交 `responsesUpstream: excel`。
   省略字段会保留已有账号配置，不等于关闭开关。
3. 已开始的请求按原快照完成，新请求走 Codex。不要中断正在执行的工具。
   旧 Excel `previous_response_id` 不能跨入口续聊，应由客户端明确创建新会话。
4. 从使用日志确认新请求不再出现 `excel_http_sse`；保留原来的调度、并发、
   代理、IPv6、指纹和账号身份配置，不为停用 Excel 修改它们。
5. 如果启用了图片中转，在旧请求排空后移除其公网转发及
   `openai.excel_image_relay_public_url` 配置。默认未启用时无须操作。

关闭 Excel 不会重启旧 State，也不清空账号或其他 CPR 缓存。
图片生成/编辑请求也由选中账号的 Excel 开关选择后端，关闭后恢复 Codex。
搜索和凭据刷新不属于 Excel 路由。

## 代码移除边界

以下路径均相对仓库根目录。执行时先搜索全部引用，再分层删除；
不能仅删除目录后用默认值掩盖遗漏的调用。

| 范围 | 定位 | 移除规则 |
| --- | --- | --- |
| Excel 协议实现 | `backend/crates/providers/openai/src/transport/excel/` | 正文/请求头、工具包装、SSE 转换、结构化输出、历史回放及压缩裁剪、图片工具后端、上传和中转均属于 Excel |
| Provider 入口 | `backend/crates/providers/openai/src/provider/excel.rs` | 删除 Excel 准备和错误分类；保留 Codex 原分类 |
| 共享文件中的 Excel 分支 | OpenAI `provider/{mod,execution,compact,observation}.rs`、`transport/{catalog,client_sse,protocol/responses}.rs`、`credential/{catalog,selector/mod,selector/capacity_wait}.rs`、`lib.rs`、`config.rs` | 只移除 Excel 字段/分支和装配，不整文件回退，不恢复 State 门槛 |
| 账号配置 | Core `account/responses_upstream.rs` 及 `account/model.rs`；Admin/API/Store 中的 `responsesUpstream`、`excelModels` | 删除路由选择及其读写映射，保留其他账号配置、导入、重登和 CAS 规则 |
| 前端 | 账号菜单、编辑、批量编辑、头像标识，以及 API 类型和相应 composables | 只移除 Excel 选项、模型输入和标识；不恢复旧 State 面板 |
| 历史缓存 | `gateway-core/src/provider_ports/replay.rs`、`gateway-store/src/redis/provider_replay.rs` 及端口装配 | 当前用于 Excel；确认无新增消费者后再删除，不动原会话粘性和交付索引 |
| 图片能力接口 | `gateway-api/src/image_relay.rs`、Core `provider_ports/image_relay.rs`、API/Provider bundle、`gateway/src/bootstrap.rs` | 确认没有其他消费者后移除临时图片接口和装配，不删除独立图片生成/编辑接口 |
| 执行入口标识 | Core `engine/{mod,coordinator}.rs` 中的 `provider_route` 和 `freeze_provider_route` | 当前用于入口冻结；确认无其他消费者后清理，不改变选择评分或重试预算 |
| 传输日志标识 | Core `event.rs` 的 `ExcelHttpSse` 和前端日志映射 | 停止新增 Excel 记录后仍需兼容历史值，不能令旧使用记录反序列化失败 |
| 依赖 | OpenAI `Cargo.toml` 中的 `jsonschema`、`imagesize`、reqwest `multipart` | 搜索其他使用者后再删；通过 Cargo 更新锁文件，不整份回退 |
| 验证与文档 | Excel 专项测试、共享测试中的新增断言、本说明和 Trellis Excel 合同 | 删除仅针对 Excel 的测试，保留原生链路及旧 State 已退役的回归 |

定位命令：

```sh
rg -n 'ResponsesUpstream|ExcelModels|responses_upstream|responsesUpstream|excel_models|excelModels' backend frontend
rg -n 'excel_http_sse|ExcelHttpSse|basispoints|transport::excel|excel_image_relay' backend frontend
rg -n 'ProviderReplay|TemporaryImage|provider_route|freeze_provider_route|with_image_relay|with_replay' backend
```

## 数据和缓存

- 已应用的 `0039_account_responses_upstream.sql`、
  `0040_account_excel_models.sql`、`0041_excel_global_models.sql` 及
  `.frozen-sha256` 必须保留，不能删改或重编号。
  删除运行时代码时允许数据库保留不再使用的列。
- 若以后确实需要删除列，另加向前迁移，并单独审核备份、混合版本兼容、
  账号导入和回滚方案；不要与普通功能停用混在一起。
- Excel 历史使用配置命名空间下的 `{provider-replay-v1}` 数据、到期索引和
  容量索引。停用后可等待到期，不执行 `FLUSHDB`，不清空共享 Redis。
  如需立即清理，须先确认没有新消费者或仍在写入的实例，再单独授权处理。
- 保留旧 State 历史迁移和兼容存储。它们不代表要恢复采集 worker、
  State 调度门槛、注入或 WS 版本约束。
- 保留账号凭据版本、Cookie 更新保护、重登/导入身份验证、缓存编号、
  账号容量预算、智能粘性、出口和指纹逻辑。

## 移除验收

1. 移除前保存当前源码 SHA 和运行版本，验证备份；在授权测试环境进行。
2. 验证所有生成请求走原生 Codex，HTTP/SSE、下游 WS 两轮及重连、
   原生续聊、compact、工具回传和取消释放容量均正常。
3. 验证导入/覆盖/重登不改变身份、分组、模型权限、代理、额度和预算，
   调度评分、粘性、账号并发及缓存编号不因删除 Excel 改变。
4. 验证旧 Excel 响应 ID 明确失败且不跨入口借用；历史使用日志仍可读取。
5. 确认无 Excel 上游请求、图片临时服务或旧 State 采集任务复活。
6. 执行后端 workspace 测试与 Clippy、前端测试/类型检查/构建、
   迁移冻结校验和隐私检查，再提交独立的 `refactor(excel)` 移除 PR。
7. 部署另行授权，按现有已验证镜像发布流程执行，不直接换回旧版本容器。
