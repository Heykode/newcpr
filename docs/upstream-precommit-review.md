# 官方改动适配与取舍

## 基线与本轮范围

- 本地主分支基线：New CPR `2abd81d0`（已对齐 Excel 设置拆分及图片策略 #116）；工作分支 `codex/upstream-precommit-controls`。
- 官方审查基线：`zyycn/codex-proxy-rs` main `d83d3eb`。
- 实现：#301、#304 加后台可调阈值、`8a07599` 的概览静默刷新部分。
- 对测但未修改生产代码：`a5a844a`。
- 仅评估：插件体系、#303/#306/#308/#312/#318/#322/#327、依赖、自更新和测试提速。
- 本轮不推送主分支、不部署、不接触生产数据库、不使用账号导出或真实上游额度。

## 已实现的行为

### #301：额度查询诊断

来源：`4634d8d`。在原有额度查询错误上保留 HTTP 状态和有界错误码，区分
401/token_revoked、一般 401、403、429、5xx 与无响应场景。公开提示是静态文案，
不是把上游原文直接展示出去。没有修改原有账号调度、凭据终态、额度事实权威或时间戳。
这不是修复上游真实额度耗尽，也不因查询接口拒绝就推断 refresh token 永久失效。

### #304：可调提交前缓冲

来源：`3bf7030`，按用户要求扩展现有后台设置。

位置：设置 → 通用与 Codex 配置 → 共享重试与原生连接高级参数 → 提交前缓冲阈值（KiB）。

| 输入 | 实际字节阈值 | 行为 |
| --- | ---: | --- |
| 默认 128 | 131072 | 使用官方扩展后的缓冲窗口 |
| 0 | 0 | 解析出的客户端事件不再额外等待 |
| 256 | 262144 | 使用保存的自定义阈值，不钳制回默认值 |
| 0.5 | 512 | 精确换算，不做整数 KiB 截断 |

非零阈值仍保留官方 2.5 秒最大等待；文字、工具语义输出和终态立即释放。
计数依据原始上游 chunk，超过阈值就放行，最后一个 chunk 可跨越阈值。
这不是请求大小限制，也不是硬性内存上限。0 不会把非流式请求强制改成流式。
设置随请求计划冻结，保存不改变已经在途的请求。新增释放原因诊断，不记录请求正文。
设为 0 的代价是结构性事件也会更早下发：一旦已经向客户端交付，后续错误不再具备
无感重试/换号条件。默认窗口扩大只让符合既有策略的早期失败有更多恢复机会，
没有增加路由尝试次数上限、预留容量或修改账号打分，但实际尝试次数可能随失败发生
时点而不同。它不是“完全没有行为变化”，也不能据此保证上游风控结果。

### 概览静默刷新

来源：`8a07599` 的 `useDashboard.ts` 部分。静默刷新不再切入加载态，失败保留旧内容
和既有错误，成功才更新；同一时刻仍只发一个概览请求，刷新频率保持 30 秒。
没有移植不属于我们现有界面的客户端身份编辑器，也没有修改客户端身份逻辑。

## a5a844a 对测结论

官方改动取消显式的连接池和 TCP/H2 keepalive 设置。测试候选保留 New CPR 的
native TLS、自定义 CA、IPv4 和禁止重定向策略，只去掉这些连接参数覆盖。
生产 `transport/client.rs` 与 `transport/egress/mod.rs` 未修改。

使用现有合成 TLS 回环测试：macOS/aarch64，每种配置 18 个请求、并发 4、4 波请求。

| 协议 | 当前配置使用连接数 | 官方默认候选使用连接数 | 结果 |
| --- | ---: | ---: | --- |
| HTTP/1.1 | 4 | 4 | 两者均流式输出并复用连接 |
| HTTP/2 | 1 | 1 | 两者均在同一 TLS 连接上多路复用 |

本地短时样本不能证明官方候选净收益更高。因此不修改默认。
这次没有验证生产 Linux、真实上游、长空闲 PING/连接回收、所有代理及 IPv6 组合，
也没有做风控成功率推断。以后是否修改，应依据隔离环境的长时和真实出口测试。

## 插件和未合并 PR 的解释

| 项目 | 真正用途 | 本轮结论 |
| --- | --- | --- |
| #326 | 提供托管上游适配接口；插件可接入自定义协议/上游，同时复用 Core 的鉴权、账号租约、重试和结算 | 不是 Excel 专用适配器。Excel/BPS 是使用场景之一；不替换我们现有 Excel 实现 |
| #306 | 给 Guardian 审批请求预留账号并发、提高排队优先级 | 当前不需要。会减少普通请求可用容量，且要适配我们 Redis 等待租约；不合 |
| #308 | Live 语音的 WebRTC 引导、sideband 和 hangup | 尚不能视为适配 New CPR 后的成熟功能；暂不合 |
| #318 | 默认关闭的 turn metadata `workspaces` 清理 | 可选隐私功能，不是设备指纹清洗或防封保证；暂不合 |
| #322 | OMP Responses 路径别名、明确拒绝不支持的 steering、HTTP 默认非流式 | HTTP 默认行为我们已有。其余两个可独立拆分，不改变生成链路；当前不为未知客户端需求引入 |
| #303 | 接管、同步、对齐、释放下游 Client Key 的周预算窗口 | 面向订阅计费周期，不是恢复上游账号额度；当前不需要 |
| #312 | 多个下游 Client Key 共享预算、并发、RPM 和队列 | 不是“接入多个上游 API Key”。按用户的单纯号池需求不合 |
| #327 | 整合 #303、#312、#324，处理它们与新主线的适配 | 不是一个独立插件，也不等于整套插件系统；当前不合 |

### #308 具体需要适配什么

审阅分支 `c41bbfa`：`provider/live.rs` 的 sideband 连接直接使用
`account.outbound_proxy()`，挂断走独立调用。不能据此认定它已覆盖我们的统一账号出口、
Mihomo 和 IPv6 路径。引导、sideband、hangup 都必须使用既有账号身份和出口约束。
通话归属还依赖进程内注册表和 TTL，需要考虑重启、多实例和排空。
WebRTC 媒体由客户端直连上游，不是 CPR 在中间转发音频。
这不说明上游实现必然有漏洞，而是不能整包加进我们这里后直接宣称不影响现有链路。

### #318 的具体边界

审阅分支 `1e1548b`：仅处理 turn metadata 顶层的 `workspaces` 字段，字段名按
trim 后不区分大小写匹配；覆盖相关 HTTP 头、`client_metadata` 和 WS 传递路径。
不删除 prompt/input 里的所有本地路径，不重建设备 ID，不改 UA。
开启会改变上游实际收到的 metadata，所以不把“剥离更多”自动等同于更安全。

### #303 与 #327 的边界

审阅整合分支 `7784618`。`ChangeWeeklyBudget.id` 的类型是 `ClientApiKeyId`：
此前“账号周预算”的说法不够准确，实际管理的是下游 Key 的本地预算周期。
例如将某客户 Key 的周预算起止对齐到其订阅周期；接管还包含控制者归属、版本校验
和是否清空本地已用量的显式语义。它不会改变 OpenAI 真实额度，也不是账号调度算法。
#327 整合 #303/#312/#324，不包含 #317；不要按四个独立功能重复合入。

### 是否需要插件体系

目前不建议为这些功能整套引入。核心调度、设备身份、账号出口、续接、计费结算仍应
在核心模块中统一维护。只读报表、外部通知、运营自动化可使用稳定管理 API 或插件；
新增独立上游有明确需求后再评估托管适配器。能否通过插件添加某项能力，取决于公开
扩展接口，不能认为安装插件体系后所有核心功能都无需修改。

## 其他维护改动

- 官方 #315 的 jsonschema 0.57.0 和 #314 中 headers 0.4.2，我们已经使用，不重复更新。
- Redis 1.7.1、config/thiserror 补丁升级可作为独立依赖更新；Redis 要跑真实 ACL、
  等待租约、取消和重连测试。AWS/S3 依赖涉及额外传递升级，独立验证存储功能。
- TLS、HTTP、WS 指纹相关的固定依赖不随普通依赖批量升级。
- Node 构建镜像 digest 更新单独走前端构建与镜像产物检查。
- `bb85945` 解决裸二进制自更新后重启路径的问题；现有 Docker 更新链不依赖它，暂不引入。
- `b3fa532` 不只是测试提速，还改 Core 连接时钟、额度时钟与 SSE 扫描。
  可单独研究测试夹具复用；不能为了测试快就连同生产时钟和解析器整体搬入。

## 已完成验证

- 以下结果均在对齐 `2abd81d0` 的 Excel 设置/图片策略之后复跑。
- Backend workspace 的所有 targets 通过 Clippy（`-D warnings`）；Rust 格式检查通过。
- Provider 定向联合组 247 项通过，覆盖预提交、额度、续接、身份、HTTP 对测、Excel、请求头、UA 与取消。
- Excel 适配器单元测试 191 项通过，包含图片策略、回放、加密恢复、工具格式与出口隔离。
- 管理层定向组 39 项、API 定向组 46 项通过；真实隔离 PostgreSQL 的设置/快照等 55 项通过，未跳过数据库连接。
- 前端设置及刷新 39 项通过；类型检查和修改文件的 ESLint 通过。
- 浏览器在 1440、390、320 宽度验证 KiB→字节换算、0、自定义值、无新增 max 属性和无横向溢出；所有 API/外部网络均阻断。
- 临时 PostgreSQL 已停止；没有接触生产实例。

复现入口（均使用现有项目工具链）：

```sh
# backend/
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo test -p provider-openai --test main -- precommit quota continuation identity_isolation http_transport_review excel headers profile smart cancellation interrupt --test-threads=4
cargo test -p provider-openai --lib -- excel --test-threads=4
# CPR_TEST_DATABASE_URL 必须指向独立测试数据库；本次使用仅监听本机 Unix socket 的临时 PostgreSQL。
cargo test -p gateway-admin -p gateway-api -p gateway-store --test main -- settings snapshot --test-threads=4
# frontend/
node --test tests/runtime-settings.test.mjs tests/dashboard-refresh.test.mjs
node node_modules/vue-tsc/bin/vue-tsc.js -b --pretty false
node tests/browser/stream-prefetch.mjs
```

本机 `pnpm exec` 报 `packages field missing or empty`，因此使用现有 `node_modules` 的
直接 Node 入口，没有改工作区配置或依赖。浏览器脚本接受 `PLAYWRIGHT_MODULE` 和
`CHROME_PATH`；本次指定已安装的 Playwright 与 Chrome，没有下载浏览器。

这些结论是本轮本地验证，不等于已部署、已完成 OVH/Linux 全链路或保证零缺陷。
