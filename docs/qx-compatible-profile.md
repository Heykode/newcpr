# QX 兼容出站画像

## 当前统一实现

本文以下“历史设计”描述迁移 0018 之前的实现，不再是当前使用说明。
现在只保留一套出站行为，删除 TLS 和会话策略下拉框及对应运行分支：

| 项目 | 当前规则 |
| --- | --- |
| HTTP/SSE | `reqwest/native-tls`，ALPN 优先提供 `h2`，同时提供 `http/1.1` |
| WS | 原 CPR Rustls、连接池和压缩机制，不把 WS 改成 HTTP/2 |
| 会话与缓存 | 固定使用账号/可信下游 Key 隔离，最终缓存键按线程派生 |
| UA | 自动更新的默认 UA，或者一个输入框填写完整 Desktop/CLI UA |
| 自定义 CA | 可选、追加信任根；配置错误拒绝，不切换 TLS 后端 |
| 调度与设备 | 权重、粘性、容量等待、重试 owner、持久设备档案保持原机制 |

默认设置提交 `{"mode":"default"}`；自定义提交
`{"mode":"custom","userAgent":"完整 UA"}`。空白值不等于默认，预览不保存。
`qx-compatible`、`independent` 及 TLS/session 选择字段不再接受，回读也不再输出
`tlsProfile`、`sessionPolicy`、`qxDefaultUserAgent`。前后端须同步升级。

Desktop 自定义 UA 的辅助桌面请求使用配套 Desktop 身份；CLI 的辅助请求继续使用
默认 Desktop 身份。自定义始终固定，恢复默认后才继续自动更新。`verified` 只描述
默认制品 UA 的选择，不表示 TLS、客户端仿真或风控核验。

HTTP 压缩仍在身份投影后按最终 UTF-8 字节数判断：不足 1024 字节普通 JSON，
否则 ZSTD level 3。WS 仍按协商采用 128/512 字节门槛及级别 6。统一客户端禁用
reqwest 自动重试；协议协商不增加业务错误降级重放。精确续接仍优先原 socket owner，
同账号仅重连不改变派生编号；换账号/Key 改号。没有明确编号时保留既有弱内容回退。

迁移 0018 在事务中先归档 mode、UA、TLS、session 和更新时间到 `legacy_selection`。
后续保存不覆盖备份，不修改账号/设备/凭据表。旧 QX 无 UA 转为其固定参考 CLI UA；
已有自定义保留原文，旧 independent 的 null UA 转为自动更新 default。
旧连接随进程升级排空，首次编号变化可能造成缓存冷启动。
回退需要旧二进制及匹配的数据库恢复方案，不能只换回旧二进制或删除迁移历史。

本次明确改变 HTTP ALPN，不宣称 ClientHello 不变或等同 QX Go；WS 保持原 Rustls。
平台/库版本仍影响 TLS。本地与隔离 Linux 测试不等于真实上游、生产吞吐或风控验收。

## 历史设计（0018 之前）

## 选择与边界

管理端全局出站设置将 UA、TLS 与会话策略独立保存，保留旧模式 API 的读写兼容。
QX 兼容仅参考无插件 SUB2-QX v1.1.26，不使用后续版本、独立插件、Go runtime
或本机转发服务。当前选择为 Provider 全局设置；按用户要求不做账号级覆盖。

QX 兼容使用 Rust 异步 HTTP/WS 与 OpenSSL 公开接口。CPR 原生 HTTP 直接使用
`reqwest/native-tls`，CPR 原生 WS 仍使用 Rustls；后台不增加第三个 TLS 选项。
QX 是功能兼容画像，不是逐项相同的 Go TLS 指纹，也不保证相同风控效果。
原型的算法排列、扩展排列、扩展 45 和密钥 share 关联差异记录在任务研究中。
产品使用 Cargo.lock 锁定的 vendored OpenSSL，不能将其他库版本的实验当作本构建验收。

| TLS 选择 | HTTP | WebSocket |
| --- | --- | --- |
| CPR 原生（`cpr`） | `reqwest/native-tls`，不启用 ALPN；通常协商 HTTP/1.1 | 原有 Rustls 与 WS 连接池 |
| QX 兼容（`qx-compatible`） | 原有自定义 OpenSSL/Hyper，支持协商 HTTP/2 | 原有自定义 OpenSSL 与 WS 连接池 |

Native TLS 使用平台后端；Linux 发布版使用项目锁定的 OpenSSL。选择同类后端不代表
不同操作系统、库版本的 ClientHello 完全一致。此次采用官方 CPR 提交 `0977de37`
的默认 HTTP 后端选择，但不整体降级依赖或覆盖 QX 实现。
自定义 CA 按 `CODEX_CA_CERTIFICATE` 优先、`SSL_CERT_FILE` 次之追加；配置错误时拒绝，
不会关闭证书校验，也不因配置 CA 将 CPR HTTP 悄悄切回 Rustls。

## UA 设置

接口沿用 `GET/POST /api/admin/settings/openai-user-agent` 和 `/preview`：

新界面始终提交完整独立选择。勾选“使用默认 UA，自动更新”时 `userAgent=null`，
仅 UA 跟随 CPR 的已核验版本更新，TLS 与会话策略均不改变：

```json
{"mode":"independent","userAgent":null,"tlsProfile":"qx-compatible","sessionPolicy":"native"}
```

取消勾选后，一个输入框自动识别受支持的 Desktop/CLI 完整格式，自定义版本固定：

```json
{"mode":"independent","userAgent":"codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color","tlsProfile":"cpr","sessionPolicy":"qx-compatible"}
```

`tlsProfile` 为 `cpr` / `qx-compatible`，`sessionPolicy` 为 `native` / `qx-compatible`，
两项必填，不能从 UA 猜测。空字符串非法，不等于恢复默认。预览不写配置。
以下旧命令仍保持原语义，不应拿旧 `default` 命令模拟新界面只切默认 UA 的操作：

```json
{"mode":"default"}
```

```json
{"mode":"custom","userAgent":"Codex Desktop/0.146.0 (Mac OS 15.7.1; arm64) unknown (Codex Desktop; 26.901.1)"}
```

```json
{"mode":"qx-compatible"}
```

```json
{"mode":"qx-compatible","userAgent":"codex_cli_rs/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color"}
```

旧 `custom` 只接受 Desktop，旧 `qx-compatible` 接受 CLI；
新 `independent` 的统一解析器同时接受两种语法，检查名称、版本及可选后缀是否一致。
识别 UA 不切换 TLS 或会话策略，允许两种 UA 与两种 TLS 的全部四种组合。
自定义版本由管理员明确指定，不跟随 CPR Desktop 制品更新。
示例版本只展示格式，不是建议追随的最新版本。

QX 无自定义 UA 时使用参考版本的
`codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color`。
QX 模式的 `verified=false`；制品核验与 TLS 等价是不同事实。

## 请求行为

- HTTP 和 WS 使用所选传输；不改变 WS 优先策略、账号权重、智能粘性或并发上限。
- 两种会话策略的 Responses HTTP 均在身份投影完成后，按最终 JSON 的 UTF-8 字节数
  决定压缩：不足 1024 字节发送普通 JSON，达到 1024 字节使用 ZSTD level 3。
  仅实际压缩时发送 `Content-Encoding: zstd`；响应仍增量读取。
  此规则独立于 TLS、UA 和会话策略，不需要新增后台设置，不扩展到额度或 OAuth 请求。
- WS 保留既有 `permessage-deflate` 握手和压缩级别 6。允许客户端字典复用时，
  不足 128 字节的消息不压缩；上游协商 `client_no_context_takeover` 时门槛为
  512 字节。未协商压缩则不压缩。门槛按最终消息字节数计算，随物理连接保存；
  小消息不会推进压缩字典，不改入站解压、控制帧、连接池归属或重试边界。
- 一次请求冻结画像，允许的 HTTP 回退使用同一选择。精确 WS 续接保留原 socket、
  原账号、原出口及原画像；切换设置不迁移已存在的连接内会话。
- 账号设备档案不变，不另建 QX installation ID。内部调度锚点保持原始值；
  QX 策略在账号选定后，用用途标识、可信下游 Key ID、持久设备编号、工作区及原始标记
  分别派生会话/线程编号，并把最终 `prompt_cache_key` 统一为线程编号。
  换账号或换 Key 改号；同账号仅重连不改号。不以 Token 或可重建的本地行 ID 算号。
  原生策略仍尊重客户端缓存键。编号变化可能导致缓存冷启动，不保证原命中率。
- 原始种子保存在有归属的 Provider 会话状态中，不保存正文副本。续接缺标记时复用种子，
  重试不会把已生成编号再算一次；错误 Key 的新状态在发送前拒绝。
  父子线程共享线程用途域，但线程自身分开；窗口保留数字代际。
- 内容回退统一识别字符串、用户消息和等价文本块，并区分工具与非文本内容。
  这会改变旧的字符串输入仅按 instructions 归组的弱亲和键；显式标记和已恢复的
  本地对话不迁移。内容相同不能证明会话归属，严格分开独立对话仍需客户端明确编号。
- WS 池在账号、出口、画像及原连接维度之外加入下游 Key；
  新建、旧 owner 查找及 tombstone 均检查。QX 策略还区分出站会话/线程，
  避免新线程复用带旧线程 opening 头的连接。精确续接仍服从原 socket owner。
- 已判定进入新轮次时，内部状态、透传头、已知正文/元数据及可解析的嵌入副本一起清理；
  不递归删除未知业务内容，不清空完整历史、工具结果或加密内容。
- 两种画像的 Responses HTTP 和 WS opening 均发送账号权威的
  `x-codex-installation-id`。编号缺失或全空白时省略，非法头值在发送前拒绝；
  下游同名头不能覆盖。此规则不扩散到 Models、额度、OAuth 或 raw JSON 入口，
  不生成新设备，也不改变正文设备字段名；已打开的 WS 不逐轮重发 opening 头。
- Token 请求保留刷新、授权码、PAT 各自语义。可能已经发送的刷新请求不能因底层
  错误类型改变而被当作未发送自动重试。
- 模型、额度、统计和下载按各自请求类型处理。头像、外部 CDN 及 Desktop 制品
  请求不能无证据换成 CLI 身份；外部 CDN 不携带 ChatGPT 凭据。
- QX 的额度服务使用另一个 Chrome 模拟客户端，不是本次参考的聊天 Go TLS。
  额度、额度重置卡、统计及头像在 QX 选择下仍保留 CPR Desktop 画像和 CPR 原生 HTTP；
  本次没有引入第三套 Chrome 模拟栈。
- 显式代理、代理认证、自定义 CA、证书与主机名校验和 IPv6 绑定仍受原有约束。
  代理与本地显式 IPv6 不可同时启用；没有可用 IPv6 不能静默切换 IPv4。

## 回退与验证

独立选择 `tlsProfile=cpr`、`sessionPolicy=native` 可让后续独立请求使用当前 CPR 原生传输/会话路径，
是否恢复默认 UA 单独控制；旧 WS 续接仍服从原 owner。
该选项不是旧 Rustls HTTP 的回退开关；还原旧 HTTP 实现需要回退代码版本。
迁移 0012 扩展原表，旧迁移字节不变。降级旧二进制前须先使用其支持的旧命令保存选择，
不能假设旧程序能读取 `independent` 或 `qx-compatible`。不要删除已应用迁移。

所有本地验证仅使用合成数据和隔离服务，不代表生产上游接受度、生产压测、
跨平台发布或风控验证。实际测试状态以本任务验收记录为准。
