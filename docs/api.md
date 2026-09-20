# Codex Proxy RS 接口

本文列出 v3 源码中的公开 HTTP 接口，路由以
`backend/crates/gateway-api/src` 中的 router 为准。配置 Codex 请先看 [客户端配置](../deploy/README.md#客户端配置)；
运行实例是否包含这些功能，应结合其版本和 revision 确认。

## 1. 鉴权与公共约定

### 客户端接口

所有 `/v1/*` 请求都使用管理端创建的 Client Key：

```http
Authorization: Bearer sk_...
```

Codex 原生生图配置还会携带 `X-OpenAI-Actor-Authorization: proxy-managed`。
它仅用于客户端识别服务端托管认证，不能代替 Client Key。网关和 OpenAI Provider 都会过滤该请求头，
上游账号身份只由服务端选中的账号提供；不要把真实账号 token 放进该标记。

Client Key 通过账号分组限定路由范围：未绑定分组时可使用全部账号，绑定一个或多个分组时只能使用
已启用分组成员的并集。分组可以混合 `openai` 与 `xai` 账号；同一请求只会在模型能力明确匹配且满足
重放安全边界时跨 Provider fallback。

运行设置可以分别配置 `minCodexDesktopVersion` 与 `minCodexCliVersion`。两者只接受 SemVer，`null`
表示不限制。API 在 Client Key 鉴权成功后识别官方 Desktop/CLI 请求头；已识别客户端没有合法版本，或版本
低于对应门槛时，所有 `/v1/*` HTTP 请求和新 WebSocket 握手在访问上游前返回 `426 Upgrade Required`。
未知客户端保持兼容，不应用版本门禁。

低版本响应使用 OpenAI 风格错误格式：

```json
{
  "error": {
    "message": "Codex CLI 0.151.0 is below the minimum required version 0.152.0. Upgrade Codex CLI and retry.",
    "type": "invalid_request_error",
    "code": "client_version_too_old",
    "client": "codex_cli",
    "current_version": "0.151.0",
    "min_version": "0.152.0"
  }
}
```

已识别但缺失或携带非法版本时，`code` 为 `client_version_unavailable`，`current_version` 为 `null`。

### 管理接口

除登录、会话状态和登出外，所有 `/api/admin/*` 请求都需要以下任一鉴权方式：

- 浏览器登录后得到的 `cpr_admin_session` Cookie；
- `x-api-key: <admin-api-key>`。

请求无需自带 `x-request-id`；缺失时服务端自动生成 UUID 并在响应头回传同一 request ID。
`api.request_id_header` 可改变注入与回传的 header 名，管理端鉴权不依赖该名字。
管理端响应统一带 `Cache-Control: no-store`。

配置了 CORS 白名单 origin 时，跨域请求以凭据模式放行，仅允许 `GET`/`POST` 方法和
`authorization`、`content-type`、`x-api-key` 与 request ID 四个请求头，不使用通配符。

普通成功响应使用以下信封：

```json
{
  "code": 200,
  "message": "OK",
  "data": {}
}
```

所有 `/api/admin/*` 错误（包括 JSON/Query rejection、未知路由和错误 HTTP method）统一返回
`application/json`：

```json
{
  "code": 40001,
  "message": "请求参数不合法",
  "data": null
}
```

管理端本地产生的 `message` 是可安全展示的中文文案；Store、Serde、Provider 内部 `Display` 和原始上游
body 不进入这个通用信封。稳定业务码如下：

| HTTP | `code` | 含义 |
| ---: | ---: | --- |
| 400 | `40000` | 请求体不是合法 JSON |
| 400 / 405 / 415 / 422 | `40001` | 通用请求、方法、Content-Type 或字段错误；HTTP 状态保留具体语义 |
| 400 | `40002` | 时间范围不合法 |
| 401 | `40101` / `40102` / `40103` | 缺少管理员会话 / 登录凭据错误 / 管理 API Key 错误 |
| 404 | `40401` | 资源或管理接口不存在 |
| 409 | `40901` | 资源状态冲突 |
| 429 | `42901` | 登录尝试过多 |
| 500 | `50001` | 服务内部错误 |
| 502 | `50201` | 上游服务请求失败 |
| 502 | `50202` | 不可逆上游操作的执行结果未知；刷新状态后再决定是否重试 |
| 503 | `50301` | 依赖服务暂不可用 |

未知 `/api/admin/*` 路径使用 `40401`，不会落入 SPA；已存在路径使用错误 method 时返回 `405`、
`40001`，并保留标准 `Allow` header。request ID 继续通过配置的响应 header 返回。

### 管理写入一致性

管理写入不要求客户端提供全局配置版本。会改变路由快照或安全配置的写入由后端在事务内推进
内部 `config_revision`，并用于快照发布与审计。账号更新和分组查询/写入的部分响应会返回
`configRevision` 作为已提交事实，但它不是客户端 mutation 的前置条件。

## 2. 健康检查

| 方法 | 路由 | 鉴权 | 说明 |
| --- | --- | --- | --- |
| `GET` | `/healthz` | 无 | Core、Store 和后台任务健康时返回 `204`，否则返回 `503` |

## 3. OpenAI 数据面与模型目录

OpenAI 模型目录是发现结果，不是推理白名单。列表尚未包含的新模型可经原有模型映射、
Client Key 账号范围及调度规则发往上游；实际不支持的模型仍由上游返回错误。
此规则同时适用于即时选账号和账号并发等待，不放宽账号权限、健康或额度限制。

除下述 JSON 入站解压和 multipart 上传保护外，Responses、Images 和 standalone Search HTTP body、
WebSocket message 和 frame 不设置网关私有长度上限；协议可接受性由上游决定。

| 方法 | 路由 | 说明 |
| --- | --- | --- |
| `POST` | `/v1/responses` | OpenAI Responses JSON；`stream=true` 返回 SSE，否则返回完整 JSON |
| `GET` | `/v1/responses` | 通过 HTTP Upgrade 建立 Responses WebSocket |
| `POST` | `/v1/chat/completions` | Chat 请求转 Responses，再将 JSON/SSE 结果转回 Chat；共用原执行链路 |
| `POST` | `/v1/responses/compact` | 显式上下文压缩；模型准入后走独立 HTTP JSON，不自动调用 |
| `POST` | `/v1/alpha/search` | Codex standalone web search；JSON 请求与响应正文原样转发 |
| `POST` | `/v1/images/generations` | 通过 OpenAI Provider 发起图像生成；JSON 请求与响应正文原样转发 |
| `POST` | `/v1/images/edits` | JSON 原样转发；multipart 文件转换为现有 JSON 图片输入 |
| `GET` | `/v1/models` | 返回当前 Client Key 账号范围内各 Provider 的可用公开模型并集；有两种响应形态，见下 |
| `GET` | `/v1/models/{model_id}` | 返回 OpenAI 兼容的单模型详情 |

Codex 的 review 等子代理请求仍使用 `/v1/responses`，并通过 `x-openai-subagent` 请求头携带子代理类型；
网关不提供独立的子代理请求路径。

HTTP Responses 只有布尔 `stream=true` 才向下游返回 SSE；省略 `stream`、显式
`false` 或非布尔值都走完整 JSON 交付，原字段保留给既有 Provider 规范化与校验路径。
该选择只决定下游交付方式，不强制改变上游传输，也不会向原始请求体补写 `stream=false`。
OpenAI Provider 沿用既有 WebSocket/HTTP 选择，将上游请求副本规范为流式，并在收齐后返回
完整结果、工具调用及用量。显式 `use_websocket=false` 仍选择 HTTP/SSE；该本地开关不进入上游请求体。
下游 WebSocket 帧仍遵循原来的流式协议，不接受 `stream=false`。

### Chat 兼容边界

Chat 默认 `stream=false`，使用与 Responses 相同的鉴权、模型范围、账号选择、并发、
粘性、代理身份、WS/HTTP 策略及结算；不调用本机 HTTP，也不另建调度或会话数据库。
`use_websocket` 仍只控制上游传输，`stream_options.include_usage` 只控制下游用量分片。

用户消息支持 `{"type":"file","file":{...}}`，转换为 Responses `input_file`；
`file_id` 与 `file_data` 必须且只能提供一个非空字符串，可附加 `filename`。
网关不下载文件，也不保证所选上游账号有权访问文件 ID。
助手历史支持字符串 `reasoning_content` 或 `reasoning`，作为普通公开历史文字发送，
不构造加密推理状态；同时给出不同值时拒绝歧义。旧式 `functions` / `function_call`
协议仍不接入，避免只有请求转换而没有对应返回语义。

支持文本、图片输入、普通 function 工具定义/多轮结果/并行调用、结构化输出、推理配置、
拒绝信息、网页搜索选项，以及显式 `web_search`、`image_generation`、
嵌套或 CPA 扁平形式的 `custom` 工具定义和多轮调用结果。custom 输出按 CPA 方式使用
`type:function` 和 `function.arguments` 外壳，内容仍是原始自由文本，不保证是 JSON。
后续请求必须携带对应 custom 工具声明，才能按名称及调用 ID 还原为上游 custom 调用和结果；
不根据参数内容猜测工具类型，歧义名称明确拒绝。保持名称、调用 ID、原始字符串、缓存和推理用量；
缺失的用量不填假零。输出 Token 上限与 temperature 在纯协议层可映射，但既有 Codex
Provider 会移除 `max_output_tokens` 和 `temperature`，因此不能保证这些控制生效；
本次保留旧转发行为，不把中间 JSON 存在字段视为上游支持。

无法表达的音频、旧式 functions、多个候选结果、stop 等请求语义明确返回 400。
显式画图工具按 CPA 方式通过 `message.images` / `delta.images` 返回独立的
`image_url` 条目，不向正文插入 Markdown 或图片编码。预览和已完成图片可以流式返回，
预览不算成功，仍须正常终态。同一图片按 item ID 比较前一次编码内容，相同则跳过；
不同内容仍发送，不缓存全部图片历史。流式每个图片分片的 `index` 从零开始。
Chat 输出层不再单独解码/检查 base64，也没有额外的 64 MiB 或图片版本数量限制；
未知短格式默认 PNG，含 `/` 的 MIME 按上游值使用。传输层和图片上传限制不变。
图片尺寸只校验 `auto` 或正整数 `宽x高`，具体模型支持哪些尺寸仍由上游决定。
搜索执行记录、来源列表及独立引用字段不转换、不追加到正文，只保留模型原文。
原生 Responses 的引用字段仍然透明转发。
因此 JSON 格式的正文不因搜索或图片而改变。不会自动重建历史图片：
assistant 的 `images` 是仅输出扩展，需要继续带图时由客户端明确发送图片输入。

namespace、MCP/电脑操作审批等需要完整专用语义的调用仍应使用原生 `/v1/responses`；
Chat 不支持的请求声明仍会拒绝，未映射的返回事件按 CPA 方式跳过。
未开始输出时转换失败返回 JSON 502；
流式输出开始后发送 `data: {"error":...}`，随后结束，不伪造正常完成或用量。
流式首包遇到心跳、创建事件、空增量或未知事件时继续等待有效输出，不先发空角色分片。
正文和工具参数逐段返回，不缓存全文来比对或补发终态快照。工具未发过参数时，
完成事件可补发整段参数；工具项目结束后的迟到增量忽略，不让整次请求报错。
缺少终态、明确失败和内部收尾失败仍不能伪装为正常完成或发送成功用量。
真实空终态不再被 Chat 自加的空输出校验拒绝；用量、费用及防重仍由原系统处理。
上游错误正文统一为 Chat 可解析的 JSON，并保留有效错误状态和可转发的重试/诊断头。

New API 的 Chat 转换只在后续发布验收后对 CPR 渠道停用。本次不修改其源码或在线配置。
New API Messages 转 Chat 的步骤仍由 New API 完成，CPR 不新增 Messages 或输入 Token
预计算入口。任何已经在 New API 转换时丢失的字段，CPR 无法恢复。
本地 New API `5e384f16` 的真实 DTO 往返验证确认：custom 的 function 外壳
可保留；类型化请求重整会丢失图片工具的附加选项及扁平 custom 声明字段，嵌套 custom
声明可保留。类型化响应重整会丢失 `images`，图片需使用透明转发，避免开启“强制格式化”
或“思考转正文”。这只是接入条件说明，本次没有修改线上渠道。客户端能否显示 `images`、
真实账号是否获准使用相应工具，需要另行验收，不能用本地模拟结果代替。

### 显式上下文压缩

`POST /v1/responses/compact` 要求非空 `model` 和字符串/数组 `input`，允许
`instructions` 字符串或 null。普通请求不会自动触发它，不增加每次请求的预计算开销。
该操作经正常模型及账号准入后，使用账号绑定的 HTTP JSON 请求 `/codex/responses/compact`；
模型映射、代理、设备身份和用量结算复用现有实现。

响应原始 JSON 和不透明 `compaction.encrypted_content` 保留，可作为后续 Responses
输入。非 null `previous_response_id` 明确拒绝，要求提交完整输入，避免无法确认所属账号时
跨账号续接。`stream`、`background`、`use_websocket` 在此入口不支持。
收到空/损坏的压缩结果不得成功；缺失用量不得猜成零费用。
New API 若在请求投影时移除 tools/reasoning/text 等字段，此入口不能恢复它们。

### 图片上传

multipart `/v1/images/edits` 在鉴权后解析 `image`、`image[]` 或 `image[数字]`，
最多 16 张 PNG/JPEG/WebP，遮罩 `mask` 最多一张且为 PNG。单部分最多 50 MiB，
整个表单最多 64 MiB，超限 413；图片签名与媒体类型须一致。要求非空 model/prompt，
未知、重复或冲突字段返回 400；暂不接受压缩 multipart 或流式图片响应。

上传图片转换为 `images[].image_url` 的 data URL，遮罩为 `mask.image_url`，随后进入
原图片路径，不写本地文件、不主动抓取 URL、不新增编排模型。已有 JSON 请求不重建，
响应也不改变。本地模拟只验证上传转换和路由，不能证明 Codex 私有图片后端接受该格式；
真实编辑、工具权限及压缩能力仍须有可用账号后验收。

### 入站与传输

`POST /v1/responses`、Chat 和 compact 在鉴权后按 `Content-Encoding` 解压，再解析 JSON；支持单一 `gzip`、
`deflate`（zlib 封装）和 `zstd`，缺省、空值或 `identity` 直接使用原始正文。gzip 多成员与 zstd
多帧连续解码，整体展开结果最多 64 MiB，超限在继续展开前返回 `400 request_too_large`；zstd
回溯窗口同样最多 64 MiB，不能满足该限制的帧按解码失败处理。这个限制保护入站解压资源，不是
模型上下文或 Token 上限，也不新增未压缩正文的长度限制。
不支持的编码、逗号分隔的叠加编码和重复 `Content-Encoding` 头返回
`400 unsupported_content_encoding`；压缩正文损坏、截断或解压后不是合法 JSON 返回
`400 invalid_json`。本地错误不包含原始正文或解压库细节。WebSocket 文本帧不经过这条解压路径。
Responses 不透传下游的逐跳头、反代元数据（如 `cf-*`、`x-forwarded-*`、`forwarded`、`via`、
`cdn-loop`）以及 `Accept-Encoding` / `Content-Encoding`。链路元数据和编解码能力
由各段传输层独立管理；其余业务扩展头继续透传，不使用固定业务头白名单。
此规则同时适用于上游 HTTP 和 WebSocket，不影响上游响应的 `cf-ray` 等诊断信息。

另外过滤下游 `x-stainless-*`、`sec-ch-ua*`、`sec-fetch-*`、`Origin` 和 `Referer`；
这些字段描述的是下游 SDK、浏览器和页面环境，不作为网关出站身份继承。
先提取 `session_id` 的会话语义，再丢弃其原样透传值；同时存在时仍以 `session-id` 为先。
最终会话、线程和安装编号继续由原有账号/Key 隔离规则生成，包括 QX 兼容别名。
`traceparent`、`tracestate`、未知业务头及正文不受新增过滤影响，原始入站头仍供本地鉴权、
CORS 和观测读取。这不是正文匿名化或风控效果保证。

OpenAI Responses 在统一请求编码阶段，将 `input` 数组中显式指定
`type: "message"` 且 `role: "system"` 的消息转换为 `role: "developer"`。
仅修改编码副本，原始入站请求保持不变；消息内容、顺序、扩展字段和顶层
`instructions` 保留，不递归修改嵌套角色，也不修改省略 `type` 的消息。
HTTP/SSE 与 WebSocket 共用此规则，发送阶段不再重复转换角色。

Responses WebSocket 仅接受文本 `response.create`，同一连接串行执行。当前响应期间收到的后续业务帧
留在有界接收队列中，待当前响应完成终结和写出后再逐条校验、准入与执行，不因请求提前到达而断开。
这对齐 Codex 客户端 `stream_request` 持锁至本轮结束的串行行为，不表示支持额外控制消息类型。
接收队列容量为 32 个事件，超载仍关闭连接；Ping/Pong、客户端关闭和服务关闭不等待队列中的请求执行。

下游 WS 无法消费的裸 `error` 会依据已有响应快照包装为 `response.failed`；
带非成功 `status`/`status_code` 的错误，以及 `websocket_connection_limit_reached`、
`previous_response_not_found` 控制错误仍保留原样。包装不增加重试、不生成成功用量，
也不把失败记为成功。

客户端使用 HTTP/SSE 时，OpenAI Provider 仍可能选择上游 WebSocket。
客户端配置的 `supports_websockets` 只控制第一段连接，不是服务端传输策略开关。
上游在响应终态前发送 Close 1000 仍属于失败，不能按“正常关闭”计为成功。

`GET /v1/models` 默认返回 OpenAI 兼容列表 `{"object": "list", "data": [...]}`；请求携带非空
`client_version` query 参数（Codex 客户端）时改为返回 Codex 专用目录合同 `{"models": [...]}`。

Codex 专用目录中的 `context_window` 与 `max_context_window` 分别表示默认上下文窗口和可覆盖上限。
OpenAI Provider 独立传递上游目录中的对应字段；任一值未知时，对应字段仍存在且为 `null`，
不会用另一个字段补齐，网关也不通过部署配置覆盖这些值。xAI 目录只声明一个窗口，
其 Provider 继续将该值同时作为默认窗口和可覆盖上限。

OpenAI 路径保留客户端 Responses wire 语义：请求 body 的未知字段和字段顺序保持不变（受控模型
映射除外），HTTP SSE 与 WebSocket 的上游业务事件字节原样转发，response ID 按 opaque 值处理而不
假设 UUID 或固定长度；OpenAI 上游错误 envelope 和允许下发的 opaque header 值也不由 canonical
观测结果重写，前述 WS 裸错误的交付包装除外。Images 的原始 JSON 请求不读取或重建，也不要求或映射模型字段；它固定使用 OpenAI Provider，
只在原始字节之外完成账号选择、鉴权头替换和端点路由，成功与失败响应正文同样保持原始字节。
`/v1/alpha/search` 使用相同的 OpenAI Provider 原生端点边界：body（包括 `model`）不解析、不映射，
`x-codex-turn-metadata` 在移除客户端账号身份并按当前 lease 重写 installation ID 后转发；上游账号
Authorization、Cookie、account ID、originator 和 User-Agent 均由代理安全重建。xAI 是 Grok wire 与
Responses wire 之间的协议转换层，转换只在 xAI Provider 内完成。
上游结构化错误的 message/code/type 会透传给客户端，其中内嵌的账号指纹 UUID 已脱敏。模型映射是
全局精确映射，未命中时模型名原样交给候选 Provider；分组只限定账号集合，不参与模型改名。

OpenAI 明确返回 `server_is_overloaded`、`slow_down` 或模型容量不足错误时，代理在允许安全重放且
尚未交付输出的前提下，先做最多 3 次同账号指数退避，再通过现有调度换号。默认间隔从 500ms 开始，
上游 `Retry-After` 参与退避计算，单次等待不超过 8 秒；重试同时受请求总尝试次数和截止时间约束。
容量不足不扣 Smart 账号健康分，不触发 Provider 全局熔断，也不作为账号额度耗尽写入冷却状态。
最终交付的上游错误仍按上述透明边界保留原始状态码、错误码和正文。
明确额度耗尽继续走现有账号隔离与安全换号流程，
包括 WebSocket 握手返回的 429；不会因其长 `Retry-After` 而转入同账号传输恢复等待。

## 4. 管理员认证

| 方法 | 路由 | 请求 | 说明 |
| --- | --- | --- | --- |
| `POST` | `/api/admin/auth/login` | `{ username?, password }` | 创建管理员会话并设置 Cookie |
| `GET` | `/api/admin/auth/status` | 无 | 返回当前 Cookie 是否已认证 |
| `POST` | `/api/admin/auth/logout` | 无 | 删除当前会话并清除 Cookie |
| `POST` | `/api/admin/auth/password` | `{ currentPassword, newPassword }` | 仅管理员会话可修改密码，成功后撤销所有旧后台会话 |

改密必须提供当前密码，单独的管理员 API Key 不授予改密权限。新密码去除首尾空白后至少
12 个字符，原文最多 1024 字节，不接受控制字符、常见弱口令或与当前密码相同的值。
每位管理员的所有会话共用 15 分钟内 10 次改密尝试额度；存储不可用时拒绝改密。
密码与安全审计在一个 PostgreSQL 事务中提交，并检查原密码哈希未被并发修改。
后台会话绑定已加盐密码哈希的摘要，密码更改后旧会话即使仍在 Redis 中也不再通过认证。
不含该摘要的旧版后台会话需要重新登录；管理员／客户端 API Key、上游账号 Cookie 和凭据不受影响。

## 5. 账号

账号 API 使用统一路由，不存在 Provider Instance 或 Provider 专属账号路由。需要 Provider 的请求只接受
`provider: "openai" | "xai"`。

| 方法 | 路由 | 主要 query/body | 说明 |
| --- | --- | --- | --- |
| `GET` | `/api/admin/accounts` | `page`、`pageSize`、`provider`、`groupId`、`search`、`status`、排序字段 | 分页查询账号与汇总 |
| `GET` | `/api/admin/accounts/detail` | `accountId` | 查询账号详情、额度和本地用量 |
| `GET` | `/api/admin/accounts/export` | `accountIds`、`confirm=export_sensitive_accounts` | 显式导出最多 200 个账号的敏感 Provider 文档 |
| `POST` | `/api/admin/accounts/import` | `{ provider, data, settings?, outboundProxyId? }` | 导入或按上游身份更新账号，可同时应用调度、分组设置与默认代理 |
| `POST` | `/api/admin/accounts/refresh` | `{ accountId }` | 手工刷新 OAuth credential（`idToken` / `accessToken` / `refreshToken`），不刷新额度 |
| `POST` | `/api/admin/accounts/recover` | `{ accountId }` | 管理员显式清除该账号的本地错误/额度/cooldown 事实并重新启用，不访问上游 |
| `POST` | `/api/admin/accounts/rotate` | OpenAI rotation 字段 | 手工替换 OpenAI OAuth token |
| `POST` | `/api/admin/accounts/update` | `{ accountId, enabled, concurrencyLimit, weight, groupIds, outboundProxyId?, outboundProxyUrl? }` | 一次更新账号调度状态、并发上限（`null` 表示继承运行参数）、权重（1–100）、所属分组与出站代理 |
| `POST` | `/api/admin/accounts/batch-update` | `{ accountIds, enabled, concurrencyLimit, weight, groupIds, outboundProxyId?, outboundProxyUrl? }` | 一次事务统一更新所选账号的调度字段、完整分组集合与可选代理 |
| `POST` | `/api/admin/accounts/delete` | `{ provider, accountIds }` | 批量删除 1–200 个账号 |
| `GET` | `/api/admin/accounts/quota` | `accountId` | 读取当前额度，不强制访问上游 |
| `POST` | `/api/admin/accounts/quota/refresh` | `{ accountId }` | 访问 Provider 并刷新额度，同时同步额度所属状态 |
| `GET` | `/api/admin/accounts/personal-info` | `accountId` | 按需汇聚 OpenAI/Codex 官方个人资料、累计活动与订阅信息，不更新额度或 credential |
| `GET` | `/api/admin/accounts/reset-credits` | `accountId` | 查询 OpenAI 上游主动额度重置卡，不读取本地库存 |
| `POST` | `/api/admin/accounts/reset-credits` | `{ accountId, creditId?, redeemRequestId }` | 使用 UUIDv4 幂等键消费一张 OpenAI 上游重置卡 |
| `GET` | `/api/admin/accounts/models` | `accountId` | 优先读取该 Provider + 套餐的模型 cache，缺失时有限实时拉取 |
| `POST` | `/api/admin/accounts/models/refresh` | `{ accountId }` | 强制拉取最新模型并覆盖 cache |
| `GET` | `/api/admin/accounts/connection-test` | `accountId`、`modelId` | 通过 SSE 返回实时连接测试事件，不作为业务 Responses 用量记录 |
| `POST` | `/api/admin/accounts/oauth/start` | `{ provider, name, accountId?, outboundProxyId?, outboundProxyUrl? }` | 创建 OpenAI 或 xAI OAuth flow；`accountId` 表示重新授权 |
| `POST` | `/api/admin/accounts/oauth/complete` | `{ provider, flowId, callbackUrl, settings? }` | 消费 OAuth callback；首次授权可附带账号设置，重新授权保留原设置 |

账号列表支持以下稳定值：

- `provider`: `all`、`openai`、`xai`；
- `groupId`: 分组 ID、`ungrouped`，或省略以不过滤；
- `status`: `normal`、`quota_exhausted`、`rate_limited`、`disabled`、`error`；
- `sortBy`: `email`、`status`、`planType`、`usage`、`lastUsedAt`、`reloginCount`、
  `addedAt`（或 `createdAt`）、`expiresAt`；
- `sortDirection`: `asc`、`desc`。

管理目录状态优先判断启停：`enabled=false` 一律返回 `disabled`（页面显示“暂停”）；
启用账号再按凭据错误、额度耗尽、限流、正常的顺序派生状态。
`status=normal` 不包含暂停账号，`status=disabled` 包含所有关闭账号。
状态排序、数据库分页和全局 `summary` 采用相同口径，五态计数互斥且相加等于总账号数。
列表与详情复用现有状态解析器；启停不清除凭据、错误或额度记录，重新开启后展示真实状态。
“正常”不表示此刻并发空闲，也不保证满足某个请求的分组、模型或路由条件。

账号视图和 Dashboard 账号概览中的 `planType` 保留原始套餐值；`planTypeDisplay` 由后端先按 Provider 解析名称，
再统一为大驼峰格式，前端直接展示该字段，例如 `Free`、`SuperGrokPro`、`EduPlus`。
OpenAI 的 `self_serve_business_prolite` 等 Team 套餐显示为 `Business`；新套餐也使用相同格式。
账号套餐为空或 `unknown` 时，后端优先用已保存的上游额度响应
中的明确套餐值补全 `planType` 和 `planTypeDisplay`；两处均无套餐信息时才显示“未知套餐”。

账号列表、详情及返回完整账号视图的刷新接口包含只读 `effectiveConcurrencyLimit`：
账号配置了 `concurrencyLimit` 时使用该覆盖值，否则继承本次查询读取的运行参数
`maxConcurrentPerAccount`。同一列表使用同一份默认值，后续查询会读取全局参数的变更；
原 `concurrencyLimit` 仍为可空配置字段，继承时保持 `null`，不会将有效值写回账号。
有效上限为 1–4294967295 的整数，`0` 不表示无限制，也不是合法配置。
该字段表示配置容量，不是剩余容量；停用、冷却或异常不会把它改成 `0`。

账号列表、详情及返回完整账号视图的刷新接口还包含只读 `reloginCount` 和
`lastReloginAt`（可空时间戳）。只累计失效重登功能成功登录并原位替换已有号池
账号凭证的次数；首次获取、首次入池、未推送、失败、普通 Token 刷新和手工 JSON
导入不累计。手动与自动重登使用同一统计，重复推送不重复计数。
计数从此功能启用起记录，不回填猜测历史；修改或删除 2FA 资料不清零，
账号原位重新导入保留计数，删除号池账号后新建则开启新的统计。

`GET /api/admin/relogin` 的每条资料同样返回 `reloginCount`、`lastReloginAt`，
对应的 `reloginAccountId`，以及只读布尔值 `hasTotp`。`hasTotp` 只表示该资料的
密码与 TOTP 材料通过本地格式校验，不返回或派生任何密钥内容；账号列表用它按
邮箱大小写无关匹配并展示 2FA 标记。按已锁定身份/工作区、明确工作区选择或唯一邮箱
候选关联号池统计；同邮箱多工作区未明确目标时计数为 `null`，不能将其当作 0
或合计值。尚未入池的资料计数为 0，账号 ID 和最近成功时间为空。
凭证替换、成功事件去重和计数在同一数据库事务中提交；号池已提交而重登资料库
收尾失败时，计数仍保留，但资料状态仍按原合同进入“推送待核实”，不自动重放。
已有账号推送在提交前完成凭据格式与身份检查；此阶段失败只返回操作错误，保留
已获取凭据及资料版本，不标为“推送结果未确认”，可在修正原因后再次确认推送。
进入提交边界后的结果不明仍按不确定结果处理，不自动重放旧凭据。仅当数据库明确
回滚了版本冲突，且重新检查确认只是 Cookie 等不改变绑定的更新时，最多尝试三次
准备与写入；仍冲突则保留缓存凭据，提示重新推送，不要求重新登录。
正常响应 Cookie 更新不会使排队任务、账号菜单确认或已获取的新凭据失效，也不会
重置自动重登的失败次数。原账号删除、用户/工作区变化或真正替换过凭据仍拦截旧任务。
此规则不改变普通 JSON 导入、账号调度开关、设备身份或已有 State 的有效期。
OpenAI 原位替换接受重登文档中的 `email`、`account_id`、`type: codex` 元数据，
邮箱和工作区必须与新令牌声明一致，元数据不能代替令牌主体或覆盖原设备身份。

重登列表新增 `importedAt`（可空）并按导入时间倒序返回，最新导入在最上方；
更新已有资料会更新导入时间，后台重登、开关调整和状态刷新不会改变该排序依据。
旧资料从有效 UUIDv7 ID 读取创建时间，无法还原时返回空值，不使用最近更新时间冒充。
`poolAccounts` 返回同邮箱的号池账号 `id`、`workspaceId`、`planType`、`enabled`、
`status`、`errorReason`、`errorMessage`，不含凭据。状态复用账号管理的运行状态解析，
暂停调度与凭据错误分别保留。缓存凭据“已验证”和历史“已同步”不能证明号池当前正常。
工作区设置只接受已知的同邮箱号池工作区或当前验证凭据的工作区；未知 ID 返回错误，
不清除现有缓存。实际登录仍需验证该工作区可访问及账号主体一致，不因本地记录绕过验证。

账号视图新增 `cumulativeCosts`，每个币种独立返回
`{ currency, estimatedAmount, estimatedAmountDisplay }`，金额沿用精确十进制字符串。
该字段独立于 `usage` 的当前额度窗口，在账号展开详情展示为“累计消费”：
同一账号原位重复导入、额度重置和请求日志清理不清零。只累计现有成功交付口径下有明确
费用的请求，一条请求只计入一次；无已知费用时为空数组，不代表零费用。
升级时从仍保留的有效历史请求补算，已经清理的历史不能恢复；删除后新建的不同账号 ID
不会自动合并历史。此统计不更改官方额度、消费计算或调度规则。

`outboundProxyId` 绑定已保存且最近测试成功的代理；省略或 `null` 保留当前绑定，空字符串清除绑定。
`outboundProxyUrl` 兼容 HTTP、HTTPS、SOCKS5、SOCKS5H 代理 URL，可带用户名和密码；不能与 ID 同时设置。
编辑时省略或 `null` 表示保持原配置，空字符串表示清除代理并直连。列表和详情只返回
不含认证信息的 `outboundProxyEndpoint`（直连时为 `null`）；只有显式敏感导出包含完整 URL。
指定代理后，推理、OAuth 服务端交换/刷新及账号辅助请求使用同一出口；代理失败不会退回直连。
浏览器打开的第三方 OAuth 授权页仍使用浏览器自身网络。
账号出口与连接隔离见 [架构说明](architecture.md#账号出站代理)。

### 独立代理管理 / Managed Proxies

所有端点要求管理员身份。所有响应只返回去掉认证信息的 `endpoint`，不会返回完整 URL。
All endpoints require admin authentication and redact proxy credentials from responses.

| 方法 / Method | 路径 / Path | 请求 / Request | 结果 / Result |
| --- | --- | --- | --- |
| `GET` | `/api/admin/proxies` | `page`、`pageSize`（1-200）、`search`（名称） | `{ items, page }` |
| `GET` | `/api/admin/proxies/accounts` | `proxyId`、`page`、`pageSize`（1-200）、`search`（账号名称或邮箱） | `{ items, page }` |
| `POST` | `/api/admin/proxies/accounts/remove` | `{ proxyId, accountId }` | `{ configRevision }` |
| `POST` | `/api/admin/proxies/create` | `{ name, proxyUrl }` | `201 { record, configRevision }` |
| `POST` | `/api/admin/proxies/update` | `{ id, revision, name, proxyUrl? }` | `{ record, configRevision }` |
| `POST` | `/api/admin/proxies/test` | `{ id, revision }` | 最新代理记录 / Proxy record with test result |
| `POST` | `/api/admin/proxies/probe` | `{ proxyUrl }` | 只探测未保存地址，返回连通性、耗时和出口 IP，不修改代理记录或账号绑定 |
| `POST` | `/api/admin/proxies/delete` | `{ id, revision }` | `{ configRevision }` |

`record` 包含 `id`、`name`、`endpoint`、`hasAuthentication`、`revision`、`accountCount`、
`lastTestAt`、`lastTest: { success, latencyMs, exitIp, message }`、`createdAt`、`updatedAt`。
未测试时 `lastTestAt` / `lastTest` 为 `null`。连通性失败返回 HTTP 200 和 `lastTest.success=false`；
记录版本过期、重复 URL、删除已绑定的代理返回 409，并发测试满载返回 429。

代理列表只返回关联账号数量。关联账号按需查询，每项包含 `id`、`name`、`email`、`provider`、`enabled`、
`authenticationKind`、`planType`、`planTypeDisplay` 和 `groups: [{ id, name, color, enabled }]`，
不返回账号凭据。默认每页 20 条，按名称、ID 稳定排序；搜索不区分大小写，匹配名称或邮箱的字面子串。
不存在的代理返回 404，未绑定账号或没有匹配结果时返回空页。数量与当前页来自同一个数据库只读快照。

移除关联账号只清除指定账号的代理绑定与连接地址，使其改为直连，保留凭据、调度参数与分组。
若账号已不再绑定请求中的代理，则返回 409；成功后在同一事务中更新配置版本与审计，并发布运行时快照。

更新省略 `proxyUrl` 保留认证；连接配置改变时清除测试结果并更新所有绑定账号。
Omit `proxyUrl` to preserve credentials. Connection changes invalidate the previous test and update all bound accounts.
Tests persist only when the requested revision still matches. Connectivity failures use HTTP 200 with
`lastTest.success=false`; stale revisions, duplicate URLs and deleting an in-use proxy return 409.
The test concurrency limit returns 429.

测试固定经代理访问双栈端点 `https://api64.ipify.org?format=json`，超时 15 秒，每进程最多同时测试 4 条。
返回本次连接实际使用的 IPv4 或 IPv6 出口地址，不分别验证两种地址族，也不代表上游账号可用。
探测器复用 OpenAI 的证书信任配置：优先读取非空的 `CODEX_CA_CERTIFICATE`，
其次读取 `SSL_CERT_FILE`，并保留系统根证书；证书配置错误不会回退为不验证证书。
出口测试通过不表示 Provider 账号权限或额度可用；账号可用性使用账号连接测试。
导入请求可以携带顶层 `outboundProxyId`，在令牌交换前解析为默认出口；文件中显式的代理配置优先。
文件及 AT/RT 导入从凭据交换到落库期间保护所选代理；此时修改、删除或写入测试结果返回 409，
避免已轮换的凭据因代理状态变化而丢失。完成导入或请求取消后自动释放保护。
OAuth 等待回调期间不持有保护；提交仍拒绝已删除、连接配置改变或测试失败的代理。

Tests reach `https://api.ipify.org?format=json` through the configured proxy, with a 15-second timeout
and four concurrent tests per process. Provider access still requires the account connection test.
Imports accept a top-level `outboundProxyId` as the default exit before token exchange; explicit per-account
settings in the document take precedence. Credential imports reserve their selected proxy until commit;
concurrent proxy mutations return 409. OAuth commits still reject a deleted, changed or failed proxy.

### 账号连接测试 SSE

`GET /api/admin/accounts/connection-test` 固定探测请求指定的账号，不参与普通账号轮换。成功流沿用
`test_start`、`request`、`content`、`test_complete` 事件；失败事件为：

```json
{
  "type": "error",
  "source": "upstream",
  "gatewayErrorCode": "rate_limited",
  "sendState": "sent",
  "error": "upstream unavailable",
  "providerErrorCode": "usage_exhausted",
  "providerErrorType": "invalid_request_error",
  "upstreamStatus": 429,
  "upstreamContentType": "application/json",
  "upstreamBody": "{\"error\":{...}}"
}
```

- `source` 为 `gateway`、`provider` 或 `upstream`：分别表示尚未进入 Provider、Provider 本地且未发送、
  已发送/可能已发送或已经捕获到上游事实。
- `gatewayErrorCode` 是 `GatewayErrorKind` 的稳定机器值，管理端据此生成中文摘要。
- `sendState` 为 `not_sent`、`sent`、`ambiguous`，非 Provider 错误为 `null`。
- `error`、`providerErrorCode`、`providerErrorType`、`upstreamStatus`、`upstreamContentType` 和
  `upstreamBody` 是实际捕获的原始诊断字段；缺失时为 `null`，不会由本地猜测或翻译。

同步导入的 `data` 必须是 JSON object，Admin API 请求上限为 64 MiB；Provider 可以收紧限制，
当前 xAI 导入上限为 16 MiB。内部 schema 由目标 Provider 独占解释：

- OpenAI 接受单账号 OAuth 文档、`accounts` 数组（最多 200 项）、CPR 账号 bundle 和含代理引用的 sub2api 导出；
- OpenAI OAuth token 字段接受 `accessToken`、`refreshToken`、`idToken`，以及官方
  `auth.json` 中的 `access_token`、`refresh_token`、`id_token`，可以嵌套在 `tokens` 等账号 object 内；
  每项至少包含 AT 或 RT。仅含 `OPENAI_API_KEY` 的客户端代理配置不是 OAuth 账号导入材料；
  RT-only 会在导入时换取 AT，AT-only 不具备自动续期能力；
- OpenAI 与 xAI 的账号条目接受 `outboundProxyUrl`；OpenAI 还会解析 sub2api 的 `proxy_key` 和顶层 `proxies`。
  代理在 token 刷新前绑定。缺失、重复、停用、带到期时间或配置回退策略的 sub2api 代理会拒绝导入；
- xAI 从单账号 object 或 `accounts` 数组中提取 OAuth token；并发、优先级等字段不参与认证；
- xAI 批量导入逐条独立校验：失败条目跳过并记录日志，不中断其余条目，仅当没有任何条目成功时整个导入才报错；
- xAI API Key 不是受支持的账号 credential；
- 导入不会只凭文件外形写入账号；目标 Provider 使用认证材料完成必要的 token exchange 或已认证账号资料补全。

管理端的 OpenAI `AT` / `RT` 标签每行一个 token，最多 200 行；每行转换为独立的
`accounts` JSON 任务条目，交给后台队列执行。Admin API 本身不接收纯文本 token 列表。
同步导入接口仍保留，例如：

```json
{
  "provider": "openai",
  "data": {
    "accounts": [
      { "accessToken": "eyJ..." },
      { "accessToken": "eyJ...", "refreshToken": "rt_...", "idToken": "eyJ..." }
    ]
  }
}
```

RT-only 使用同一形状，只提交 `refreshToken`。不得把真实 token 写入日志、issue、fixture 或文档。

账号导入与首次 OAuth complete 可附带 `settings: { enabled, concurrencyLimit, weight, groupIds }`。
提供 `settings` 时四项均必填，`concurrencyLimit: null` 继承运行参数，否则为 1–4294967295 的整数；
`weight` 为 1–100，`groupIds` 为完整分组集合。设置应用于本次导入的全部账号，包括匹配到的已有账号，
与凭据在同一事务内提交；分组不存在时整次回滚。省略 `settings` 时新账号使用默认设置并保持未分组，
已有账号保留原有分组、权重与并发设置。重新授权不接受 `settings`，普通 credential refresh/rotation 也保留账号设置。

管理端先配置账号设置，再选择 OAuth、AT/RT 或账号文件完成导入。返回设置保留输入；更改出站配置会使
旧 OAuth 链接失效。文件中显式的出站配置优先于表单代理，未指定时使用表单代理。
账号列表的每个 item 返回轻量 `groups: [{ id, name, enabled }]`。

### 受管 State

运行设置的 `turnStateInjectionEnabled`、账号的 `turnStateInjectionEnabled` 和
`turnStateModels` 模型范围共同决定是否启用。关闭停止注入及采集，不删除已有缓存或延长计时。
账号开关用 `POST /api/admin/accounts/batch-update` 的最小补丁修改，不携带其他调度字段。

运行设置的 `turnStateProbeProxyId` 选择探测出口：`null` 使用 IPv6 池，字符串引用
代理管理中已测试成功的代理 ID，不复制代理 URL 或凭据。更新请求省略此字段保留原选择，
显式 `null` 切回 IPv6。正在被选用的代理不可删除，须先切换探测出口。
代理探测复用独立 HTTP 客户端，每次新建连接，不复用业务连接池；代理失败不会回退直连。
修改代理 URL 后使用新配置重新采集，已有有效 State 不因出口变化被清空。
新建连接不保证动态代理每次提供不同公网 IP，出口分配由代理服务决定。
此设置只影响 State 采集，不改变正常用户请求的代理、身份、Cookie 或 WS 路由。

账号列表和详情的 `turnState` 只返回安全元数据：

- `enabled`：全局及账号 State 开关是否同时启用。
- `requiredModels`：设置中的受管模型；`readyModels`：当前账号可用、距离 State 到期超过一分钟的模型。
- `models[]`：`model`、`refreshStatus`、`probeAttempts`、`successfulProbeAttempt`、`lastProbeReason`、`active`、`standby`。
- `probeAttempts` 为本次采集任务累计尝试数，等待响应期间也定期更新；`successfulProbeAttempt` 为采集成功的那一次尝试序号，未成功或旧记录未知时为 `null`，不等同于并发批次总尝试数。
- 槽位为 `null` 或 `{ chars, capturedAt, expiresAt }`，不包含原始 State。旧记录采集时间未知时为 `null`。
- `standby` 沿用存储字段名，仅表示刷新期间的待切换值，不代表常驻备用。
- `refreshStatus` 为 `missing/queued/refreshing/cooldown/ready/failed`；诊断原因是安全代码，
  不是上游原文。`probeAttempts` 是本次采集任务累计发起的尝试数，不是成功数或终身总数。

关闭开关或账号异常时仍可显示绑定匹配且未过期的缓存，但 `readyModels` 为空。
字符数和回包是否相同仅是诊断事实，不保证上游接受、模型质量或实际寿命。

### 后台导入任务

| 方法 | 路径 | 作用 |
| --- | --- | --- |
| `POST` | `/api/admin/accounts/import-tasks` | 接受 `{ submissionId, items }`，返回 202 和任务摘要 |
| `GET` | `/api/admin/accounts/import-tasks` | 当前管理员的任务摘要列表 `{ items }` |
| `GET` | `/api/admin/accounts/import-tasks/detail?taskId=...` | 任务摘要及逐条结果 |
| `POST` | `/api/admin/accounts/import-tasks/stop` | `{ taskId }`，跳过未开始项，在途项继续完成 |

每项复用同步导入的 `{ provider, data, settings?, outboundProxyId? }`，不改变账号识别、
设备复用、导入事务、设置或套餐同步。JSON 文档保持整体，不在前端擅自拆开内部账号；
因此“条目成功数”与“已入库账号数”可以不同。OAuth 和重新授权仍走原接口。

任务请求最多 4 MiB、1–200 项；进程内共用 3 个执行槽位、最多 8 个活跃任务和
100 个保留任务。任务间轮转，已完成结果保留 1 小时。相同管理员及 `submissionId`
在保留期内重复提交相同内容返回原任务，内容不同返回冲突；提交 ID 不包含令牌。
请求与结果按管理员主体隔离，而不是按某一次网页请求 ID 隔离。

结果状态为待执行、执行中、成功、失败、待核对和已跳过，对应
`pending/running/succeeded/failed/unknown/skipped`；结果不包含导入凭据。
无法确定上游交换或数据库提交结果时标为 `unknown`，不自动重试。
停止任务不撤销已提交账号，也不强行中断可能正在轮换 RT 的条目。
任务由服务端持有，关闭网页不取消；服务重启不保留进程内任务及幂等记录，
重新导入前应核对已入库账号和凭据状态。

### 凭据更新

OpenAI 的 CPR 导出保持 OAuth 账号的既有 token 与过期时间字段。

OpenAI rotation 请求字段为：

```json
{
  "provider": "openai",
  "accountId": "acct_...",
  "idToken": "...",
  "accessToken": "...",
  "refreshToken": "..."
}
```

OAuth start 使用：

```json
{
  "provider": "openai",
  "name": "account name",
  "accountId": null
}
```

重新授权已有账号时，start 请求仍携带 `provider` 和展示用 `name`，只额外提供目标 `accountId`；
客户端不得提交 `credentialRevision`、旧 token 身份或其他并发控制字段。complete 请求也不重复提交
`accountId`，后端通过 `flowId` 中保存的目标绑定完成授权。

### OpenAI 身份、额度与状态

- OAuth 文件导入接受 camelCase 与 snake_case 的三个 token 字段，内部统一保存为
  `accessToken`、`refreshToken`、`idToken`，不接受含义模糊的 `token`。
  仅有 refresh token 时先换取 access token。
- 普通 OAuth 的身份补全复用官方 `token_data.rs::parse_chatgpt_jwt_claims`：优先解析 `idToken`，缺失字段再由
  `accessToken` 补齐；`email` 优先 JWT 顶层值、其次 `https://api.openai.com/profile.email`，用户 ID
  优先 `chatgpt_user_id`、其次 `user_id`。该路径不调用 `whoami`，也不信任导入文档顶层的
  `userId/accountId`。新 ID/AT 的用户或 workspace 字段明确冲突时拒绝导入，不能用优先级
  掩盖冲突；没有足够主体字段的新账号仍可按既有 unresolved 语义导入。本地解析不等于 JWT 验签。
- `at-` 开头的 Codex Personal Access Token（PAT）可直接粘贴到现有 **AT 导入** 入口，或使用
  `{"accessToken":"at-..."}` JSON；也接受官方 `auth.json` 的 `personal_access_token` 字段
  （兼容 `personalAccessToken`）。导入时向 OpenAI auth 的
  `/api/accounts/v1/user-auth-credential/whoami` 验证令牌，以响应中的用户 ID、账号 ID 和套餐建立身份，
  `email` 可缺失；不从导入文件的身份字段或附带的 ID token 回退补齐。验证失败不导入，接口区分提示
  PAT 格式无效、被上游拒绝、验证服务不可用和身份响应无效，不回显令牌或原始上游响应。
  PAT 不保存附带的 refresh token、ID token 或推测的过期时间，不参加 OAuth RT 刷新；
  失效后需取得新 PAT 再导入。普通 JWT 导入行为不变。
- 首次 OAuth 保留回调 `state`、PKCE 与官方 token exchange，并持久化 `idToken`、`accessToken`、
  `refreshToken`。刷新响应中的三个 token 字段均按官方语义独立轮换：返回新值时替换，省略时分别保留
  现值。重新授权也保留这些回调保护，但必须先检查新凭据与目标账号的主体连续性。回调地址只承载 `code`/`state`，
  不以 host/path 形式作为拒绝条件。
- 手工替换 token 和重新授权在写入前检查新 ID/AT，不能用旧 ID token 为未知新凭据背书。
  用户或 workspace 明确不一致，或缺少足以确认连续性的主体资料时返回 409，旧凭据和设备保持原样。
  提示无法确认不代表 token 无效；可以使用完整的新凭据资料或新建授权。同主体成功变更保留原设备，
  更新新凭据中存在的 email/plan，缺失时保留旧值。不按 email 匹配账号，不新增身份网络查询。
- 账号文件导入和首次 OAuth 创建在 credential 提交后立即尝试一次额度观测。观测失败只记录告警，
  不回滚已提交的账号；重新授权和手工或后台 RT 刷新不隐式等同于手工额度刷新。
  正常 RT 刷新保持已有身份与资料，不因 opaque AT 或省略未变 ID token 增加身份验证请求。
- OAuth pending flow 先取得带过期时间的独占 claim，只有账号事务提交成功后才消费。失败会释放 claim，
  但上游 authorization code 本身通常只能交换一次；已完成过 token exchange 时应重新创建 OAuth flow。
- `GET /accounts/quota` 只读取最后一次落库快照；`POST /accounts/quota/refresh` 才访问上游。access token
  已过期时，额度刷新要求先走 credential 刷新或重新授权，不会拿过期 token 探测额度。
- OpenAI 已耗尽账号每 30 分钟主动复核一次，也会在最早未恢复窗口的 `resetAt + 2 分钟` 到期后
  提前复核。后台每 30 秒检查触发条件；同一重置边界复核后仍未恢复时回到 30 分钟重试，
  避免旧 reset 持续触发请求。各窗口独立确认恢复，时间到期本身不会直接解除账号耗尽。
- OpenAI 未耗尽账号的非零用量窗口超过 `resetAt + 2 分钟` 后也会主动刷新，并沿用 30 分钟重试节流；
  零用量窗口不触发这类额外刷新。以成功上游观测更新额度，不因时间到期直接清零本地用量。
- 同一已知 `resetAt` 的账号级窗口也可通过连续两次新鲜观测确认未触顶后恢复。
  缺失窗口、未知用量、再次触顶或重置时间不匹配会中断该窗口证据；旧观测、重复观测、
  耗尽前观测不推进恢复。新一轮耗尽不复用旧进度，额度恢复不改变凭据错误或启停状态。
- `POST /accounts/recover` 是管理员对本地事实的强制恢复：它清除 Redis cooldown 和已保存的额度/错误，
  把账号重新启用并恢复为可调度 credential；它不验证上游账号是否已经恢复，下一次真实请求仍可重新写入
  失败事实。
- 成功额度观测会 revision-fenced 写入 quota；明确 `Allowed` 投影为 `normal`，明确耗尽投影为
  `quota_exhausted`。额度观测不会清除凭据过期、无效或封禁事实；这些事实统一投影为 `error`，并由
  `errorReason` 区分。额度接口的 401/403 也不足以判定 refresh token 永久失效，credential 终态只由
  OAuth refresh 的明确永久错误写入。
- 正常 Responses 请求会解析上游响应的 rate-limit headers，合并进同一 quota 快照并同步状态。Free、
  K12 等套餐共用该状态机；套餐只参与账号展示和按套餐隔离的模型目录 cache，不存在 K12 专属额度路径。
- 账号展开区的 Token 结构、模型排行和列表 Token 汇总优先使用账号级周额度窗口，无可统计的周窗口时
  使用月额度窗口；`usage.windowLabelDisplay` 随选中的窗口返回“周额度窗口”或“月额度窗口”。查询边界
  严格为 `[resetAt - windowSeconds, resetAt)`，不是自然周/月或最近 7/30 天；额度刷新若返回了更早的
  重置时间，会按新边界重新聚合。没有边界完整、可归属到账号的周/月窗口时显示无数据，标签为
  “周/月额度窗口”，不回退到 5 小时、日窗口或历史累计。各额度条与 Dashboard 的百分比选择不受影响。
  金额原值保持完整精度，USD 展示值
  小于 1 美元时最多保留四位小数，其余保留两位。
- `usage.quotaWindow` 返回上述同一消费源窗口的 `key`、`period`（`weekly`/`monthly`）、
  `usedPercent`、`resetAt` 以及后端统一计算的 `estimatedUsd`、`remainingUsd`；
  无可统计窗口时为 `null`。预计周额度使用本轮原始 USD 消费
  `* 100 / usedPercent`，只接受未过期周窗口、有限正金额和正百分比，不设 5% 门槛。
  缺失数据不借用历史累计、模型专属窗口或 Plan 学习值；列表、美元预测详情与分组监控共用
  Admin `current_window_estimate`，前端只格式化，不维护第二份公式。
- 账号列表沿用每 30 秒静默刷新，预计周额度随同一响应重新计算，不经过五分钟历史预测缓存，
  列表不再自动预取历史预测。手工额度刷新只替换响应中的账号行并同步状态汇总，不触发整页 loading；
  若新状态不符合当前筛选，该行从当前页移除。以上读取不触发上游额度或凭据刷新。

### OpenAI 出站画像

`GET/POST /api/admin/settings/openai-user-agent` 读取或保存全局出站选择；
`POST /api/admin/settings/openai-user-agent/preview` 仅校验并预览，不持久化。
模式为 `default`、`custom`。`custom` 必须提供完整 Desktop/CLI `userAgent`；
`default` 使用 CPR 默认并自动更新。TLS/session 统一执行，不接受旧
`qx-compatible`、`independent` 模式及其选择字段。迁移 0018 先备份并转换已有设置，
非空完整 UA 自动识别 Desktop/CLI，空字符串拒绝。UA、TLS、会话策略不互相推断。
回读不再包含 `tlsProfile`、`sessionPolicy`、`qxDefaultUserAgent`。
QX 兼容并不表示 TLS 指纹逐项等价；设备档案和精确 WS 续接身份保持原有权威。
详细格式、分请求类型的边界与回退方式见 [QX 兼容画像](qx-compatible-profile.md)。

### OpenAI 个人信息

`GET /api/admin/accounts/personal-info?accountId=...` 需要管理员会话，当前由 OpenAI/Codex OAuth
账号提供。后端并发读取资料统计与订阅，一次返回；每次请求均重新查询，不自动重试或
刷新 credential，不读取本地 usage/billing 记录，也不缓存或估算统计结果。

响应 `data` 包含：

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| `profile` | object 或 null | 官方个人资料、累计统计与活动洞察；查询失败为 null |
| `profileError` | string 或 null | 资料查询失败时的安全错误提示；成功为 null |
| `subscription` | object 或 null | 当前绑定账号的订阅周期；无可用周期或查询失败为 null |

两部分的查询结果独立：资料失败时仍返回可用订阅，订阅失败时仍返回资料。账号不存在、查询参数不合法或
无管理员会话时，仍返回标准错误；请求期间账号身份或 credential revision 改变时拒绝整份结果。

#### 官方资料与累计统计

`profile` 包含：

- `displayName`、`username`、`imageUrl`：官方账号资料；
- `summary`：累计文本 Token、单日峰值 Token、最长任务时长、当前连续天数和最长连续天数；
- `dailyUsage`：按日期返回的 Token 活动；
- `activityInsights`：快速模式占比、上游原样返回的推理强度及占比、Skill 探索/使用数、聊天总数，
  以及插件与 Skill 调用排行。

官方未返回的字段保持 `null`，不使用本地数据补齐；`hasStatsError: true` 表示账号资料可用，但官方统计
部分不可用。access token 已过期或官方返回 401 时，通过 `profileError` 提示先刷新 credential 或重新授权。
原账号级 `GET /api/admin/accounts/usage-statistics` usage/billing 报表接口及其查询链路已移除。

#### 订阅信息

后端使用当前凭据、绑定的上游账号 ID 和账号出站代理访问 `/backend-api/subscriptions?account_id=...`，
不枚举其他账号，不返回上游订阅 ID 或原始响应。

`subscription` 为 `null`（未获得可用订阅周期），或包含以下字段：

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| `startsAt` | RFC 3339 字符串或 null | 上游本期开始时间 |
| `expiresAt` | RFC 3339 字符串 | 上游本期结束时间，不代表自动续费账号最终失效 |
| `willRenew` | boolean 或 null | 自动续费状态；未知不推断为 false |
| `billingPeriod` | string 或 null | 上游计费周期标识 |
| `billingCurrency` | string 或 null | 上游计费币种 |
| `observedAt` | RFC 3339 字符串 | 本次查询时间 |

订阅不写入额度快照或数据库，不参与账号状态或调度；单次上游查询最多 5 秒、响应最多 64 KiB，不重试。
上游失败或未提供有效周期时返回未知，不据此标记免费、过期或禁用；请求期间账号身份或 credential
revision 变化时丢弃结果。

### OpenAI 主动额度重置卡

`GET /api/admin/accounts/reset-credits?accountId=...` 每次都查询 OpenAI 上游；后端不把卡片列表写入
PostgreSQL 或 Redis。管理端只在用户打开弹窗或点击刷新时调用，并在当前浏览器会话内缓存最近一次成功
结果，用于账号行上的 `xN` 提示。

查询响应：

```json
{
  "availableCount": 1,
  "credits": [{
    "id": "credit_...",
    "status": "available",
    "title": "...",
    "expiresAt": "2026-08-31T12:00:00Z",
    "resetType": "..."
  }]
}
```

消费请求的 `redeemRequestId` 必须是小写、带连字符的 canonical UUIDv4；`creditId` 可省略，由上游选择
可用卡。一次请求发出后若传输结果不明确，重试必须复用完全相同的 `redeemRequestId`、`creditId` 和
账号。服务在单副本进程内按账号串行消费，并在 credential 需要刷新时以同一命令重试一次；它不会对不明
结果自动创建新消费。

若服务无法确认不可逆消费是否完成，返回 HTTP `502` / 业务码 `50202`；客户端应先刷新卡片与额度状态，
并在确需重试时复用原 `redeemRequestId`。明确的上游 HTTP 拒绝仍使用 `50201`，不会误标为结果未知。

```json
{
  "accountId": "acct_...",
  "creditId": "credit_...",
  "redeemRequestId": "8fbf302d-11df-4bd5-82e4-08e4b3df7874"
}
```

消费响应只返回上游结果 `code` 和可选 `credit`。消费端确认成功后应重新 GET 卡片列表，并显式调用
`POST /api/admin/accounts/quota/refresh` 回读官方额度；不得直接改写本地 `resetAt`。xAI 不支持该能力。

## 6. 账号分组

分组是 Provider-neutral 的账号集合；一个组可包含任意 Provider 账号，一个账号也可属于多个组。

| 方法 | 路由 | 主要 query/body | 说明 |
| --- | --- | --- | --- |
| `GET` | `/api/admin/account-groups` | `page`、`pageSize`、`search`、`enabled` | 分页查询分组；返回账号可用性、并发槽位（Redis 不可用时 `usedSlots=null`）及成功请求 USD 用量 |
| `GET` | `/api/admin/account-groups/monitor` | `groupIds=grp_a,grp_b,grp_c`、可选 `refreshForecasts` | 只读监控，一次一至三个不同分组；需要管理员认证，拒绝未知字段及不存在的分组 |
| `POST` | `/api/admin/account-groups/create` | `{ name, description, color }` | 创建空分组；`color` 严格为 `#RRGGBBAA`，返回时统一大写 |
| `POST` | `/api/admin/account-groups/update` | `{ id, name, description, color }` | 更新名称、描述和颜色 |
| `POST` | `/api/admin/account-groups/enable` | `{ id }` | 启用 |
| `POST` | `/api/admin/account-groups/disable` | `{ id }` | 禁用；已绑定 Key 保持受限，不回退到全部账号 |
| `POST` | `/api/admin/account-groups/delete` | `{ id }` | 删除未被 Client Key 引用的组 |

列表数据为 `{ items, page, configRevision }`，其中 item 返回 `memberCount`、按 Provider 聚合的
`providerCounts` 和 `clientKeyCount`。查询分组成员使用账号列表的 `groupId` 筛选，
不提供独立的分组成员路由；账号的 Provider 不代表整个分组的 Provider。

监控数据为 `{ viewerScope, generatedAt, rateWindowSeconds: 60, items }`。`viewerScope` 仅用于
当前 origin 的管理员置顶偏好隔离，不含会话凭据。监控不会刷新上游额度、登录身份或写入分组配置。
后台采样会更新专用监控生命周期与快照表，不再读写额度学习表，不修改账号业务状态或计费事实。

| 监控 item 字段 | 口径 |
| --- | --- |
| `totalAccounts/eligibleAccounts/estimatedAccounts` | 成员数、可调度成员数、已知额度成员数；组内账号去重 |
| `remainingUsd/remainingStatus` | 复用账号列表的后端动态公式，汇总实际周窗口的剩余额度；不继承 Plan 额度，不用短窗口或月窗口补算 |
| `expectedExpiryUsd/expiryStatus` | 同 Provider、同套餐最近最多五个尚未恢复的确认失效账号的平均寿命所预测的浪费额度；不是 Token 有效期或额度重置 |
| `consumeUsdPerMinute` | `(generatedAt-60s, generatedAt]` 成功完成推理费用，按历史 `routing_group_refs` 授权范围归属，可跨组重叠 |
| `quotaConsumeUsdPerMinute` | 本组可调度共享账号在所有分组的分钟费用；组内账号去重 |
| `etaMinutes/etaStatus` | 剩余额度除以上述共享账号消耗；不减去预计过期额度，零消耗不返回无限时长 |
| `usedSlots/totalSlots` | 可调度账号的共享占用/动态槽位，不是分组独占并发 |
| `lowSample/earliestResetAt` | 样本不足提示、最早额度源窗口重置时间 |

状态包括 `ready/partial/learning/unknown/disabled`，ETA 另有 `idle/empty`。未知、读取失败、
缺失费用返回 `null`，已记录的零保留为 `0`。部分额度可展示已知小计，但不推算 ETA。
寿命复用现有账号 `created_at`。迁移 `0016_monitor_account_lifecycles.sql` 只保存监控生命周期：
身份使用现有 Provider/上游用户/空间标识，缺少用户标识时按本地账号 ID 隔离，不按邮箱合并。
`banned/account_banned` 立即形成样本；`invalid/credential_invalid`、`expired/credential_expired`
连续两分钟后计入，使用原状态观测时间而不是每次轮询时间。普通 access token 到期、
临时 401、网络错误、限流、额度耗尽和正常账号被删除不形成失效样本。
恢复要求当前 `ready`、token 未过期，并有开始于失效及当前凭据状态更新时间之后的成功推理；
仅清错误、重新导入不足以撤销样本。恢复后退出平均值，原创建时间保留；同 Provider/Plan
取最近最多五个尚未恢复的失效账号，较早样本可替补。确认失效历史保留跨删除，
再次失效更新同一账号记录。寿命是入池到失效的跨度，包含中间不可用时间；
超出平均寿命保持未知，不是官方失效时间承诺。

Store 统计使用同一只读重复读快照，每条 SQL 最多 2 秒。监控读取并发最多 2；
后台每 10 秒统一采样所有分组，全轮共享账号去重，当前额度加载并发最多 2、单轮最多 60 秒。
每轮复用账号列表的窗口消费查询重新计算，不读取固定学习额度；无需加载 Token 配对历史、
健康图或累计费用。寿命同步只写监控表，随后业务统计仍使用只读重复读快照。
采样通过 Admin 的独立监控 worker 接入现有 Host 生命周期，无人打开页面也运行，不修改请求调度。
采样失败仅记录监控警告、下一轮重试，不因展示预测失败而改变网关就绪状态。
新增 `0015_group_monitor_snapshots.sql`，每组只持久保存最新一份结果、配置版本和采样时间，
整轮事务发布；失败不覆盖旧结果或其时间。配置变更后等待新版本快照，删除组级联清理其快照。
前端每 10 秒只读取可见的一至三个组，后台标签页暂停，数据超过 45 秒或请求失败标记陈旧。
账号列表仍独立每 30 秒刷新。普通读取只取保存的快照，不触发采样，也不重新标记 `generatedAt`；
尚无有效快照返回 `503`。仅右上角立即刷新/重试携带 `refreshForecasts=true`（严格布尔值，默认 `false`），
触发同一个全局采样后返回当前页。重叠后台/手动调用合并，普通快照读取不等待采样锁。
按钮加载时禁用、拒绝重复点击，失败保留旧数据和当前分页，不刷新账号列表或其独立预测缓存。
移除本监控原有的 300 秒预测缓存；重算依赖 CPR 已有上游快照，不请求上游额度或身份刷新，
因此不承诺上游数据本身即时更新。历史 Plan 学习样本不清空。
仅展示有绑定账号或已置顶的组；绑定条件取 `memberCount > 0`，不要求账号正常或存在流量。
全空目录初始化时允许只读一个现有组以恢复管理员置顶偏好，此组不会自动展示或持续轮询。
分组置顶只影响本机显示顺序，不改变调度优先级；分组额度及并发不能跨组直接求和。

### 动态额度口径

每个账号的 `总额度 = 当前窗口 USD 消费 * 100 / 已用百分比`；
`剩余额度 = max(总额度 - 当前窗口消费, 0)`。金额和比例必须有限且大于零，
窗口必须有效，不设 `$5`、`3` 或 `5` 个百分点门槛。

- 分组只汇总真实 7D 源窗口的动态剩余，不用短窗口/月折算或 Plan 模板兜底。
  这不保证可连续使用到耗尽，短期限额仍可能阻断使用。
- 列表每 30 秒读取时即时计算，分组后台每 10 秒计算；相同输入使用相同函数，
  不新增个人预测缓存或一份消费账本。未知/部分数据保留覆盖状态。
- `GET /api/admin/accounts/quota-forecast` 保持原周/月格式及 Token 配对算法，
  美元值使用同一当前窗口函数；月/周折算仍标注，剩余不折算。
  详情缓存有效期 10 秒，打开时每 30 秒重新读取，不触发上游额度刷新。
- 原额度学习调用已停用；`0014` 和旧数据原样保留，运行时预测不再读写或继承。
  寿命生命周期与已停用的额度样本无关。

## 7. Client Key

| 方法 | 路由 | 主要 query/body | 说明 |
| --- | --- | --- | --- |
| `GET` | `/api/admin/client-keys` | `cursor`、`limit`、`search`、`sortBy`、`sortDirection` | 游标分页查询 |
| `POST` | `/api/admin/client-keys/create` | 创建字段 | 创建带账号范围的 Client Key |
| `GET` | `/api/admin/client-keys/reveal` | `id` | 显式读取完整明文 Key |
| `POST` | `/api/admin/client-keys/update` | 更新字段 | 原子更新名称、分组范围和限额 |
| `POST` | `/api/admin/client-keys/enable` | `{ id }` | 启用 |
| `POST` | `/api/admin/client-keys/disable` | `{ id }` | 禁用 |
| `POST` | `/api/admin/client-keys/delete` | `{ id }` | 删除 |
| `POST` | `/api/admin/client-keys/reset-budget` | `{ id, period: "daily" | "weekly" | "all" }` | 清零指定日／周窗口的已用金额 |

创建字段为 `name`、可选 `label`、`groupIds`、`maxConcurrency`、`requestsPerMinute`、可选
`dailyLimitUsd` 和 `weeklyLimitUsd`，更新请求再增加
`id`。`groupIds` 必须显式提交：空数组派生 `routingScope: "all"`，非空数组派生
`routingScope: "groups"`。响应同时返回分组引用 `groups`，以及从当前有效账号池派生、仅供展示的
`providerKinds`；Client Key 不再保存 `providerKind`。创建和 reveal 响应会返回完整明文 Key，调用方
必须立即安全保存。

金额字段为非负十进制字符串，最多 10 位整数与 10 位小数，`"0"` 表示不限额。
创建时省略金额字段默认为零；更新时省略或 `null` 保留当前值，修改限额不会清空已用金额。
`maxConcurrency` 和 `requestsPerMinute` 是非负整数，零表示不限。

列表增加 `dailyLimitUsd`、`weeklyLimitUsd`、`dailyUsedUsd`、`weeklyUsedUsd`（均为字符串）、
`dailyResetsAt`、`weeklyResetsAt`（RFC3339 或 `null`）。
管理端日／周金额显示两位小数，悬停可查看原始值；记账和限额比较保留完整精度。
日窗口按北京时间零点重置；周窗口从首次准入当天零点起持续七天，到期后在下一次使用时重新开启。
费用按请求完成时间归属窗口。并发按同一 Key 的执行中请求累计，包含 SSE 与每个 WebSocket
`response.create`；空闲连接不占名额，内部重试不重复占用。
修改 Key 策略对既有 WebSocket 连接的下一次请求同样生效，已开始的请求保持原有快照。

任一已结算金额达到限额后拒绝新请求，已准入请求可完成并使金额超过阈值。
HTTP 返回 `429`，`error.code` 为 `key_daily_budget_exceeded` 或 `key_weekly_budget_exceeded`，
并附 `Retry-After`；WebSocket 每次 `response.create` 执行相同检查并返回协议错误事件。
只累计上游上报或按用量与模型价格计算出的 USD 费用；无法取得费用的尝试按零累计，
保留错误和用量诊断，不产生待核账记录或阻断。内部重试中已经取得的费用仍会累计。
预算存储不可用时返回 `503`、`key_budget_unavailable`。

自动结算按网关请求 ID 幂等执行。账本独立于使用统计日志，记录保留至删除 Key，
不受 `usageRetentionDays` 影响。

手动重置预算与准入、结算共用 Key 行锁，并在同一个事务内写审计。不更改限额、到期时间、
启停状态或历史费用记录；从未使用的 Key 不因此开启窗口。所选有效窗口的计费起点推进至重置
时刻，重置前完成但延迟结算的请求不再回扣该窗口，重置后完成的请求照常计费。
前端不自动重试重置操作；响应不明确时先刷新列表确认已用金额，再决定是否重新操作。

## 8. 运行设置

| 方法 | 路由 | 说明 |
| --- | --- | --- |
| `GET` | `/api/admin/settings` | 读取运行设置 |
| `POST` | `/api/admin/settings/update` | 原子替换全部运行设置 |
| `GET` | `/api/admin/settings/client-downloads/codex-desktop/windows` | 提取 Codex Desktop Windows 离线安装直链；`refresh=true` 强制刷新进程内短缓存 |
| `GET` | `/api/admin/settings/admin-api-key` | 只返回管理 API Key 是否存在 |
| `POST` | `/api/admin/settings/admin-api-key/delete` | 删除管理 API Key |
| `POST` | `/api/admin/settings/admin-api-key/regenerate` | 重新生成并一次性返回完整管理 API Key |

设置更新字段包括：

```text
modelMappings
refreshMarginSeconds
refreshConcurrency
maxConcurrentPerAccount
requestIntervalMs
rotationStrategy
minCodexDesktopVersion
minCodexCliVersion
usageRetentionDays
opsEventRetentionDays
auditRetentionDays
requestTuning
```

`rotationStrategy` 可取 `smart`、`quota_reset_priority`、`round_robin`、`sticky`。
两个 `minCodex*Version` 字段为 `string | null`，只设置最低版本，不存在最大版本字段。

新增的独立参数同样位于 `requestTuning`：

| 字段 | 默认 | 作用 |
| --- | --- | --- |
| `openaiLocationOverrideEnabled` | `false` | 是否覆盖 OpenAI 搜索地区和环境上下文时区 |
| `maxWaitingPerKey` | `0` | 单个下游 Key 的等待人数上限，整数 0–1024；0 关闭 |
| `keyConcurrencyWaitTimeoutSeconds` | `30` | Key 并发排队最长时间，整数 1–600 秒 |

地区开关关闭时保留客户端原值；开启时使用配置文件的 `openai.wire_profile.location`，
未自定义时为 `US / Ohio / Piketon / America/New_York`。开关不清空地区配置，
不更换 UA、TLS、账号设备档案或系统时区。已开始请求使用冻结的开关值。

Key 排队只处理并发已满，不等待 RPM 或预算额度恢复。队列按进程维护，
同 Key 先进先出，各 Key 共用每进程 1024 个等待名额；Redis 仍是全局并发准入权威，
多实例之间不保证全局 FIFO。等待不重复记 RPM，不提前返回成功，不占上游账号。
获得名额后继续原调度；如还需要等待账号，沿用剩余等待截止时间，不叠加完整新预算。
排队满或超时返回 HTTP 429，错误码分别为 `concurrency_queue_full`、
`concurrency_queue_timeout`。已取消的准入通过有时限的 Redis 标记防止迟到写入重新占位；
清理故障时仍依赖租约到期收敛，不声称网络故障下立即释放。

上述字段缺失或为 `null` 时继承默认值。升级不会自动开启地区覆盖或 Key 排队。
降级到不认识这些字段的旧版本前，应备份并移除这三个新增设置字段。

`requestTuning` 中的 OpenAI 账号忙时等待参数：

| 字段 | 默认 | 范围 |
| --- | --- | --- |
| `accountBusyWaitEnabled` | `false` | boolean |
| `accountBusyWaitStickyMaxWaiting` | `3` | 整数 1–1000 |
| `accountBusyWaitStickyTimeoutSeconds` | `120` | 整数 1–600 秒 |
| `accountBusyWaitFallbackMaxWaiting` | `100` | 整数 1–1000 |
| `accountBusyWaitFallbackTimeoutSeconds` | `30` | 整数 1–600 秒 |

缺失或 `null` 使用默认，`false` 不被默认值覆盖；零不表示无限等待。设置更新仍是全量替换，
应保留其他 `requestTuning` 字段。关闭后新请求使用原有快速选择路径；已开始的请求保持冻结设置。
只有健康、授权范围内且仅执行并发暂时满载的账号可等待，请求间隔、冷却、停用、失效或无额度不进入队列。
两个模式共用每账号等待总人数，不是全池 100 人，也不增加执行并发。原号队列满时普通软粘性可另选账号；
精确续接仍只能使用原 owner。等待受请求总截止及共享阶段预算限制，换号不重置，阶段超时不叠加新预算。
等待期间不向上游发送业务请求，不提前提交 SSE；名额获得后继续原有 HTTP/WS 传输。
Redis 故障拒绝准入，取消正常释放登记；故障或进程退出时由原截止时间限制残留。

关闭开关不等于可直接降级旧二进制：旧配置解析器不认识上述五个字段。降级前先备份设置，
仅移除这五个新增字段并保留其他运行设置，再切换版本。

`requestTuning.websocketLargeRequestThresholdBytes` 控制 OpenAI 普通新链的大请求发送前 HTTP 选择：

- 默认 `15728640`（15 MiB），允许整数 `0..67108864`；`0` 关闭大小分流，缺失或 `null` 继承默认。
- 仅当 `websocketHttpFallbackEnabled=true` 且传输要求为 `new_chain` 时生效。
  比较的是身份投影、协议归一化后的最终 WS JSON UTF-8 字节数，达到阈值即走现有 HTTP/SSE 通道，
  不建立 WS、不尝试先发送失败再重放。HTTP 沿用选定账号、设备画像、出口与下游交付偏好。
- 原生 WS 的 `store=false` 首轮、预热及所有已有 `previous_response_id` 的续链不参与大小分流。
  不拼接历史，不自动压缩会话，不将该决定写成会话永久 HTTP 状态；后续独立小请求仍按原策略选择。
- 这是保守路由阈值，不是上游硬限额或 HTTP 无限大小保证。WS 压缩不改变此字节计算。
  已发送后的 WS 1000/1009 仍沿用发送状态与禁止不确定重放的既有规则。
- 诊断 `transport.fallback` 记录 `decision=http_large_request`、`payloadBytes`、`thresholdBytes`
  和传输要求，不新增正文采集。运行设置仍全量替换，保存时保留其他字段，进行中的请求保持冻结值。
- 降级到不认识该字段的旧版本前，先备份并从持久化 `requestTuning` 中移除该字段；设为 `0`
  只关闭功能，不会使旧配置解析器认识该字段。

Windows 离线包接口固定解析 Microsoft Store Product ID `9PLM9XGG6VKS` 的 Retail 包，不接受调用方提供
产品 ID、上游地址、ring 或文件名。后端只返回通过包名、架构、Microsoft CDN host/path、scheme 和失效
时间校验的 `x64` / `arm64` MSIX 直链，不代理安装包字节。Store 内容通道返回 HTTP/80 临时地址时保留
原始 scheme，不强制改写为该 host 不保证支持的 HTTPS。动态链接不足 10 分钟即失效时不会下发；某个架构
解析失败时只将该架构降级到 OpenAI 官方 HTTPS 稳定 MSIX，并通过 `warning` 说明。响应形状为：

```json
{
  "resolvedAt": "2026-09-01T06:30:00Z",
  "cached": false,
  "warning": null,
  "packages": [
    {
      "architecture": "x64",
      "source": "microsoft_store",
      "version": "26.825.6671.0",
      "fileName": "OpenAI.Codex_26.825.6671.0_x64__2p2nqsd0c76g0.msix",
      "sizeBytes": 744250000,
      "downloadUrl": "http://dl.delivery.mp.microsoft.com/filestreamingservice/files/...",
      "expiresAt": "2026-09-01T07:30:00Z"
    }
  ]
}
```

`source` 为 `microsoft_store` 或 `official_openai`。Store 的四段 package version 只用于下载展示，不参与
Desktop 三段 SemVer 门禁，也不会自动回写最低版本设置。门禁规则见
[鉴权与公共约定](#1-鉴权与公共约定)，解析器职责见 [架构文档](architecture.md#11-生命周期安全与恢复)。

## 9. 备份

全部备份端点位于 `/api/admin/settings/backups/*`，内部由独立 BackupService 承担，不并入设置用例。响应继续使用 `AdminEnvelope`，wire 字段 camelCase，`Cache-Control: no-store`。

| 方法 | 路由 | 请求 | 说明 |
| --- | --- | --- | --- |
| `GET` | `/api/admin/settings/backups` | 无 | 读取存储配置（含明文 Secret）、验证状态与调度配置 |
| `POST` | `/api/admin/settings/backups/storage/update` | S3 配置 | 更新存储配置；`secretAccessKey` 为空字符串会校验失败 |
| `POST` | `/api/admin/settings/backups/storage/test` | 无 | 测试已保存的存储配置（Put/Head/Get/Delete 探针） |
| `POST` | `/api/admin/settings/backups/schedule/update` | 调度配置 | 更新 Cron、时区与保留策略 |
| `GET` | `/api/admin/settings/backups/records` | 查询参数 | 分页查询备份记录 |
| `POST` | `/api/admin/settings/backups/create` | `{ expiresInDays? }` | 创建手动备份，返回 `202 Accepted`；`expiresInDays` 为过期天数（0 或缺省表示不过期） |
| `POST` | `/api/admin/settings/backups/download-url` | `{ backupId }` | 创建 5 分钟有效预签名下载地址（仅 completed） |
| `POST` | `/api/admin/settings/backups/delete` | `{ backupId }` | 请求删除（进入 `deleting`，由 Worker 收敛硬删除） |

读取设置响应（Secret 以明文返回，由前端掩码显示）：

```text
storageRevision, endpoint, region, bucket, accessKeyId, secretAccessKey, prefix,
forcePathStyle, verified, scheduleEnabled, cronExpression, scheduleTimezone,
retentionDays, retentionCount, nextRunAt, lastVerifiedAt, updatedAt
```

更新存储请求字段：

```text
endpoint, region, bucket, accessKeyId, secretAccessKey, prefix, forcePathStyle
```

`secretAccessKey` 为空字符串会校验失败；由于 GET 会回传已保存的明文 Secret，保存时始终整体提交当前值。已有备份记录时，endpoint/region/bucket/forcePathStyle 不允许变化（存储身份锁定，`409`）；只允许轮换凭据与修改 prefix。

保存相同配置保留验证状态、定时计划及配置版本。存储配置实际变化时，会同时使验证失效、暂停定时计划并清空下次运行时间；连接测试通过后需重新启用计划。

更新调度请求字段：

```text
scheduleEnabled, cronExpression, scheduleTimezone, retentionDays, retentionCount
```

`cronExpression` 为 5 段格式；`retentionDays`/`retentionCount` 为 0 表示禁用对应清理。启用计划前必须已保存完整存储配置且通过连接测试。

记录列表查询参数：

```text
page, pageSize, status, trigger
```

`status` 可取 `queued/dumping/uploading/completed/failed/deleting`；`trigger` 可取 `manual/scheduled`。记录响应字段：

```text
id, triggerKind, status, scheduledAt, objectKey, sizeBytes, sha256, attemptCount,
errorCode, errorMessage, startedAt, completedAt, expiresAt, createdAt, updatedAt
```

`expiresAt` 在创建时确定：手动备份来自 `expiresInDays`，计划备份来自当时的
`retentionDays`；到期后由 Worker 进入删除流程。

连接测试响应：

```text
{ ok, stage, code, message }
```

`stage` 为 `putObject/headObject/getObject/deleteObject`。探测成功后以 `storageRevision` CAS 写入 `lastVerifiedAt`；测试期间配置变化则丢弃结果。

备份错误映射（`AdminErrorCode` 既有体系）：

| HTTP | 场景 |
| --- | --- |
| `400` | 配置、Cron、时区或状态参数无效 |
| `404` | 备份记录不存在 |
| `409` | 已有活跃任务、状态冲突或存储身份锁定 |
| `502` | S3 兼容服务返回无效或失败响应 |
| `503` | PostgreSQL、`pg_dump` 或对象存储暂不可用 |

审计动作：`backup.s3_config_updated`、`backup.s3_connection_tested`、`backup.schedule_updated`、`backup.created`、`backup.download_url_created`、`backup.delete_requested`。审计详情与记录表均不保存 Secret、数据库连接串或预签名 URL query。

## 10. Dashboard、用量与错误

| 方法 | 路由 | 说明 |
| --- | --- | --- |
| `GET` | `/api/admin/dashboard/summary` | Dashboard 汇总；支持 `kind`、`startTime`、`endTime` |
| `GET` | `/api/admin/dashboard/trend` | Dashboard 趋势；`kind=usage|latency|errors` |
| `GET` | `/api/admin/usage/records` | 请求记录分页列表 |
| `GET` | `/api/admin/usage/records/detail` | 按 `id` 查询请求详情 |
| `GET` | `/api/admin/usage/records/summary` | 当前筛选条件的请求汇总 |
| `GET` | `/api/admin/usage/insights/overview` | 用量、成本与成功率洞察 |
| `GET` | `/api/admin/usage/insights/diagnostics` | 按维度聚合诊断 |
| `GET` | `/api/admin/operations/errors` | 运维错误分页列表 |

用量查询可组合页码/游标、时间范围、Provider、Client Key、账号、模型、route、transport、状态码、
request/response/upstream ID、outcome 与搜索文本。诊断 `dimension` 可取 `model`、`account`、
`apiKey`、`provider`、`transport`、`failureClass`、`status`。

汇总与洞察中的请求数与 outcome 分布覆盖筛选范围内全部请求；token、缓存、延迟与成本聚合仅统计
已完整交付客户端的成功响应。

详情接口按 `id` 可读取成功、失败或未完成请求。新增 `trace`（历史未采集记录为 `null`）和
`relatedRequests[]`（`requestId / relation / outcome / completedAt`）；`relation` 为 `recovered_by` 或
`recovers`。`trace` 是执行终态时的有界脱敏时间线，包含 request、attempt 和 exchange 关联、阶段、
事件摘要及淘汰计数；普通用量列表不携带此字段。

错误记录中的“已自动恢复”表示系统关联到了后续成功请求，不会把原来的失败记录改为成功。
`upstreamSendState = ambiguous` 表示无法确认该次上游执行结果，不代表后续恢复请求失败；
恢复关联也不等于逐字节验证过两次请求正文。

Dashboard 的 `accountUsage[]` 由后端提供 `usageWindow`、`metricLabel`、`metricValue`。
`usageWindow` 复用账号额度窗口合同，缺失额度事实时为 `null`；窗口标签、百分比、触顶状态、重置时间
和本地用量由 Provider/Admin 投影。前端不得从套餐缺失推断免费套餐，也不得从显示时舍入的百分比推断
触顶。滚动窗口使用相应时间范围的本地用量，独立于 Dashboard 的今日统计范围。

OpenAI 的 `serviceTier` 只接受上游响应生命周期事件确认的实际 `response.service_tier`；请求里的
期望档位只保留在 request summary，不能冒充响应事实。计费展示把 `priority`/`fast` 映射为 `Fast`，
`flex` 映射为 `Flex`，缺失或 `default` 映射为 `Default`；未知非空值原样展示。Fast 优先使用模型的
priority 价格，缺少专用价格时回退到标准价格的 `2.00x`；Flex 为 `0.50x`，Default 为 `1.00x`。

## 11. 版本、更新与重启

| 方法 | 路由 | 主要 query/body | 说明 |
| --- | --- | --- | --- |
| `GET` | `/api/admin/system/version` | 无 | 当前构建、部署模式和可用更新 |
| `GET` | `/api/admin/system/update/detail` | `refresh=true|false` | 读取或强制刷新 Release 详情 |
| `GET` | `/api/admin/system/update/events` | 无 | SSE 更新事件流 |
| `POST` | `/api/admin/system/update` | 可选 `{ targetVersion }` | 开始在线更新 |
| `GET` | `/api/admin/system/update/status` | 无 | 查询当前更新或回滚状态 |
| `POST` | `/api/admin/system/rollback` | 无 | 回滚到保留的上一版本 |
| `POST` | `/api/admin/system/restart` | 无 | 请求进程重启 |

在线更新仅在当前部署模式、Release 资产和进程重启能力都满足要求时可用，且只在同一 major 版本内
提供：跨大版本目标会以 `40901` 冲突拒绝，需按发布说明重新部署。
实例升级和仓库发版见 [部署文档](../deploy/README.md#镜像升级与源码构建)。

## 请求地区覆盖

- 继续使用 `requestTuning.openaiLocationOverrideEnabled` 总开关，默认关闭。
- `requestTuning.openaiRequestLocation` 可保存 `{country, region, city, timezone}`。
  `null` 或未设置时沿用 `openai.wire_profile.location` 启动配置，启动默认值不变。
- 代理创建、更新和查询支持 `requestLocation`。更新时省略表示保留，`null` 表示
  清除，完整对象表示替换。国家代码必须是两个大写 ASCII 字母，地区和城市须为
  1 至 128 个非控制字符，时区必须是有效的 IANA 时区。
- 总开关开启时：选中账号所绑定代理的地区优先，其次为全局运行配置，最后为
  启动配置。总开关关闭时，代理配置也不改写客户端的地区信息。
- 只改写标记过的环境上下文日期、时区和 Web Search 位置；不改变服务器时区、
  出口连接、TLS、UA、账号设备编号、Token、Cookie 或账号并发设置。
- 地区解析发生在账号选定之后。保留原有身份和调度输入；新增全局/代理配置
  不额外改变无编号请求的缓存身份。旧有地区开关和启动地区配置对内容回退
  的影响保持不变，不能据此声称所有无编号对话的缓存键永远不变。
- 不包含上游 #126 的容量冻结、自动降低并发或恢复探测任务。
