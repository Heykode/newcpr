# 官方 v3.44.0 合入与差异账

## 固定范围

- 本地起点：`7a781b03aefb547666ade35bb0cd1557f19fe222`，分支 `codex/upstream-v3440`。
- 上次官方：`e7ef31d882deb033538a23643779a9f41328c46a`（v3.43.1）。
- 本次官方：`cf7a9bd501dcacfcf3b2f722e8f681db5af149d1`（v3.44.0），12 个提交、58 条最终变更路径。
- UI：`61ceb0c0130463510fb922a554a398dee6b71a86` → `46ade64479009ab9bc7cb1c8ec6d143fbd0a3b6c`（v0.9.1），2 个提交、9 条路径。
- 合并前生产记录：源码 `71a07c483647d47629b42cadee92d815075936d4`，对应 main `7a781b03a…`，树相同；本轮未操作生产。
- 本账为源码合入证据，不代表提交、推送、发布或部署。共用规则仍为 [官方更新合并核对清单](upstream-integration-checklist.md)。

## 执行与归因

| 官方来源 | 合入内容 | 归因及本地接入 |
| --- | --- | --- |
| `0d444923` | 普通通用额度头归 Codex，仅图片端点重归类 | 官方 v3.43.1 行为的后续修复，本地同样适用；不是上次漏合。采用官方两个 parser 及 endpoint 标志，覆盖成功/失败 |
| `16b36717` | 连接测试显示代理配置、连接、隧道提示 | OpenAI HTTP/WS、xAI → ProviderDiagnostic → Core 两条出口 → Admin Failed → SSE proxyHint → 前端单次提示。诊断本身不增加调度、重试或换号限制 |
| `5741683e` / UI `e9b3d740` | 材质切换不改变圆角和尺寸 | 四个官方实现文件移植；本地 ThemeName/ResolvedThemeSeedTokens 命名、--cp-*、持久化键和完整 themeProfiles 保留 |
| `5647dc8d` | 五组 Actions 更新 | 更新所有实际消费者，包括本地共享 action/build-cache；不新加重复 CI 阶段 |
| `462cef5d`、`47ce36f4` | Rust 依赖更新 | zip 9.4.0、async-compression 0.4.52、compression-codecs 0.4.47；host yaml-rust2 0.13.0。config 的 yaml-rust2 0.11.1 独立保留 |
| `03186b54`、`65fd3499` | Debian/Node 镜像摘要更新 | 原样采用官方固定 digest，不改部署拓扑 |
| `a708301c` | 六项前端依赖更新 | lucide 1.70.0、es-toolkit 1.53.2、Vue 3.5.45、Tailwind/vite-plugin 4.3.5、Vite 8.3.6；保留本地依赖 |
| `31f471b0` | 插件按 proxy ID 请求和目录查询 | 按既有批准边界排除；Admin 窄端口/提取 helper 仅服务插件，无普通代理功能新增，不引入无消费者代码 |
| `c622f085`、`cf7a9bd5` | 官方版本与通道元数据 | 不复制官方发行资产声明；后续本仓发布另行生成 |

## 必要适配与旧规则替换

1. 图片分桶保留本地 `CodexRateLimitObservation.observed_at`，失败仍用 `PreserveAccess`；
   普通 Responses/Live/主动额度刷新/Excel 文本及 compact 不走图片重归类。
   图片生成/编辑成功与失败均由实际 endpoint 判定，不按模型名猜测。
2. NewCPR 的普通账号和受管出口共用 `account_exit_client`，准备失败接入官方安全 mapper。
   旧代码仅附加 source，无法携带代理提示；这次采用官方错误分类入口，不额外新增限制。
   HTTP 连接提示覆盖已经选择的受管代理；WS 仍按实际拨号阶段判断，保留本地 IPv6 和预算。
3. 本地 `map_handshake_error` 继续保存 Excel 所需的真实 HTTP 拒绝状态，同时传入账号。
   Excel compact 及对应调用方同步，避免丢失 401/403/429 恢复依据。
4. Core 有 `observe_probe_failure` 和本地 `from_provider` 两条出口，均接入提示。
   后者旧逻辑只看 NotSent，会把已捕获上游状态的错误叫作 Provider 本地失败；
   按官方 `has_upstream_facts` 规则替换，只有确实本地的失败才能携带代理提示。
5. 前端删除无条件按 401/403/429/5xx 先猜原因的旧分支，使用官方 source/sendState/hint/错误码。
   失败文案放日志 `failureMessage` 并在状态区显示一次；表单与模型加载错误仍单独显示。
   旧测试改验新显示位置，不把实现恢复成重复提示。POST、自定义 prompt/stream、取消与迟到保护保留。
6. 现行额度规范废弃全局 active-limit 重归类；主题规范废弃 soft 自带尺寸覆盖。
   API 文档同步已存在的 POST 测试入口；旧版本差异账继续作为历史记录，不作为新合同。
7. 既有多分组布局、重登记录/2FA/立即重登、State、Excel 与已取消的 PRISM 边界不扩大。
   无新迁移、临时脚本或重复验证框架进入源码；临时浏览器场景和原始日志放仓库外。
8. 两条本地额度断言仍要求把普通请求的通用窗口移到 active-limit；按官方新 parser 改为
   保留 Codex 窗口和 active-limit 元数据，不恢复旧实现。
9. 既有 WebSocket 停机用例在暂停时钟下等待真实 TCP EOF，重复运行出现时过时不过。
   改为真实时钟，保留原有 1 秒超时、EOF 和 pool shutdown 断言；仅修测试时序，
   没有放宽产品截止、改动连接池或新增测试框架。
10. xAI 源文件变动使 11 条既有合成测试样例审查记录失效。逐项复核原样例及上下文未变，
    仅同步文件 SHA-256 和移位行号；无新增豁免、扫描规则或隐私门禁变更。

## 逐路径核对：官方主仓

| 官方路径 | 本地处理 |
| --- | --- |
| `.github/workflows/_quality.yml` | 原样更新 install-action 固定 SHA；保留既有按影响选择验证和制品复用 |
| `.github/workflows/release.yml` | 原样更新实际 login/Cosign/Release 引用；pnpm 位于本地共享 action，一并更新 |
| `.github/workflows/security.yml` | 落到本地 security-scan.yml；共享 build-cache 和 build-frontend 引用同批更新 |
| `backend/Cargo.lock` | 采用官方实际改动；仅适配本地格式、现有字段或测试夹具接口 |
| `backend/apps/gateway/src/bootstrap/plugins.rs` | 明确排除：插件专属端口、实现、测试或文档；不改变普通代理功能 |
| `backend/crates/gateway-admin/src/bootstrap.rs` | 明确排除：插件专属端口、实现、测试或文档；不改变普通代理功能 |
| `backend/crates/gateway-admin/src/lib.rs` | 明确排除：插件专属端口、实现、测试或文档；不改变普通代理功能 |
| `backend/crates/gateway-admin/src/model/accounts.rs` | 采用官方实际改动；仅适配本地格式、现有字段或测试夹具接口 |
| `backend/crates/gateway-admin/src/ports/proxy.rs` | 明确排除：插件专属端口、实现、测试或文档；不改变普通代理功能 |
| `backend/crates/gateway-admin/src/use_case/accounts/mod.rs` | 本地 accounts.rs，失败事件实际携带 proxy_hint |
| `backend/crates/gateway-admin/src/use_case/proxies.rs` | 明确排除：插件专属端口、实现、测试或文档；不改变普通代理功能 |
| `backend/crates/gateway-admin/tests/use_case/proxies.rs` | 明确排除：插件专属端口、实现、测试或文档；不改变普通代理功能 |
| `backend/crates/gateway-api/src/admin/accounts/wire.rs` | 采用官方实际改动；仅适配本地格式、现有字段或测试夹具接口 |
| `backend/crates/gateway-api/tests/admin/accounts/mod.rs` | 采用官方实际改动；仅适配本地格式、现有字段或测试夹具接口 |
| `backend/crates/gateway-core/src/engine/execution/service.rs` | 本地 execution.rs 的 observe_probe_failure 原样接入官方 hint 分支 |
| `backend/crates/gateway-core/src/engine/probe.rs` | 新增字段/accessor；本地 from_provider 额外出口也使用官方事实分类与 hint 规则，保留 provider_kind |
| `backend/crates/gateway-core/src/error/mod.rs` | 采用官方实际改动；仅适配本地格式、现有字段或测试夹具接口 |
| `backend/crates/gateway-core/tests/engine/execution/mod.rs` | 本地 execution.rs，采用官方 LocalFailingProvider 断言 |
| `backend/crates/gateway-host/Cargo.toml` | 采用官方实际改动；仅适配本地格式、现有字段或测试夹具接口 |
| `backend/crates/gateway-plugin/runtime/src/callback/dispatch.rs` | 明确排除：插件专属端口、实现、测试或文档；不改变普通代理功能 |
| `backend/crates/gateway-plugin/runtime/src/callback/http.rs` | 明确排除：插件专属端口、实现、测试或文档；不改变普通代理功能 |
| `backend/crates/gateway-plugin/runtime/src/callback/mod.rs` | 明确排除：插件专属端口、实现、测试或文档；不改变普通代理功能 |
| `backend/crates/gateway-plugin/runtime/src/callback/ports.rs` | 明确排除：插件专属端口、实现、测试或文档；不改变普通代理功能 |
| `backend/crates/gateway-plugin/runtime/src/callback/proxies.rs` | 明确排除：插件专属端口、实现、测试或文档；不改变普通代理功能 |
| `backend/crates/gateway-plugin/runtime/src/generation/instance.rs` | 明确排除：插件专属端口、实现、测试或文档；不改变普通代理功能 |
| `backend/crates/gateway-plugin/runtime/src/generation/runtime.rs` | 明确排除：插件专属端口、实现、测试或文档；不改变普通代理功能 |
| `backend/crates/gateway-plugin/runtime/tests/generation/data.rs` | 明确排除：插件专属端口、实现、测试或文档；不改变普通代理功能 |
| `backend/crates/gateway-plugin/sdk/docs/capabilities.md` | 明确排除：插件专属端口、实现、测试或文档；不改变普通代理功能 |
| `backend/crates/gateway-plugin/sdk/src/call/host.rs` | 明确排除：插件专属端口、实现、测试或文档；不改变普通代理功能 |
| `backend/crates/gateway-plugin/sdk/src/client/http.rs` | 明确排除：插件专属端口、实现、测试或文档；不改变普通代理功能 |
| `backend/crates/gateway-plugin/sdk/tests/client/http.rs` | 明确排除：插件专属端口、实现、测试或文档；不改变普通代理功能 |
| `backend/crates/gateway-protocol/src/openai/events.rs` | 采用官方实际改动；仅适配本地格式、现有字段或测试夹具接口 |
| `backend/crates/gateway-protocol/tests/openai/events.rs` | 采用官方实际改动；仅适配本地格式、现有字段或测试夹具接口 |
| `backend/crates/providers/openai/src/provider/execution.rs` | 图片上下文传成功/失败；握手带账号；保留 Excel image/compact 调用及观测时间 |
| `backend/crates/providers/openai/src/provider/failure.rs` | 采用官方诊断和图片上下文；保留 Excel 真 HTTP 拒绝状态、capture time 与 PreserveAccess；选中受管代理同样提示 |
| `backend/crates/providers/openai/src/provider/live.rs` | 图片上下文 false；握手带账号；准备失败走官方安全 mapper |
| `backend/crates/providers/openai/src/provider/mod.rs` | 本地已集中到 request_proxy.rs 的 account_exit_client，在实际准备入口接入官方 mapper |
| `backend/crates/providers/openai/src/provider/observation.rs` | 按图片上下文选官方 parser，再封装本地带 observed_at 的 observation；不改权限 |
| `backend/crates/providers/openai/src/transport/websocket/coordinator.rs` | 采用官方实际改动；仅适配本地格式、现有字段或测试夹具接口 |
| `backend/crates/providers/openai/src/transport/websocket/error.rs` | 采用官方结构化 Connect/ConnectTimeout；同步本地预算判断及受管出口消费者 |
| `backend/crates/providers/openai/src/transport/websocket/handshake.rs` | 采用官方阶段提示；保留本地 IPv6/source-bound 和连接预算错误，适配结构化 Connect |
| `backend/crates/providers/openai/tests/credential/quota/mod.rs` | 采用官方实际改动；仅适配本地格式、现有字段或测试夹具接口 |
| `backend/crates/providers/openai/tests/provider/contract/mod.rs` | 官方 HTTP/WS 直连、代理不可达/隧道拒绝正反例；图片既有测试扩展生成/编辑 × 成功/失败 |
| `backend/crates/providers/xai/src/transport/network.rs` | 官方 reqwest connect hint；保留本地不含 URL 的 error source |
| `backend/crates/providers/xai/tests/transport/network.rs` | 采用官方实际改动；仅适配本地格式、现有字段或测试夹具接口 |
| `deploy/Dockerfile` | 采用官方实际改动；仅适配本地格式、现有字段或测试夹具接口 |
| `docs/api.md` | 合入连接测试 proxyHint 合同；同步已有 POST 真实入口。插件段排除 |
| `docs/architecture.md` | 替换旧额度归类说明；插件段排除 |
| `docs/theme.md` | 材质仅光影，尺寸独立；保留本地 Token/主题入口说明 |
| `frontend/package.json` | 同步六个适用依赖；本地内置 UI 源码，无 @codex-proxy/ui npm 包 |
| `frontend/pnpm-lock.yaml` | pnpm 11.7.0 重新生成，保留本地额外依赖；frozen-lockfile 安装通过 |
| `frontend/src/i18n/locales/en/accounts.json` | 采用官方实际改动；仅适配本地格式、现有字段或测试夹具接口 |
| `frontend/src/i18n/locales/zh-Hans/accounts.json` | 采用官方实际改动；仅适配本地格式、现有字段或测试夹具接口 |
| `frontend/src/i18n/locales/zh-Hant/accounts.json` | 采用官方实际改动；仅适配本地格式、现有字段或测试夹具接口 |
| `frontend/src/views/accounts/composables/useAccountConnectionTest.ts` | 采用官方 hint/failureMessage 展示；保留 POST 参数、模型选择、取消/迟到隔离；删除旧的状态码抢先猜测 |
| `modules/ui` | 本地内置 UI，移植 v0.9.1 四个实现文件，详见下表 |
| `release/notes.md` | 官方说明归入本差异账；不提前写 NewCPR 已发布声明 |
| `release/version.yaml` | 记录固定版本来源；不复制官方 published SHA/资产通道，不提前发布本仓版本 |

## 逐路径核对：官方 UI

| 官方 UI 路径 | 本地处理 |
| --- | --- |
| `README.md` | UI 独立包的安装/发行元数据；本地内置源码不复制包版本或安装说明 |
| `package.json` | UI 独立包的安装/发行元数据；本地内置源码不复制包版本或安装说明 |
| `playground/guide/installation.md` | UI 独立包的安装/发行元数据；本地内置源码不复制包版本或安装说明 |
| `playground/guide/theme.md` | 主题行为说明同步到 docs/theme.md 和现行组件规范 |
| `release/notes.md` | UI 独立包的安装/发行元数据；本地内置源码不复制包版本或安装说明 |
| `src/theme/core/constants.ts` | 移植到 `frontend/src/theme/core/constants.ts`；保留本地类型名与 Token 键 |
| `src/theme/core/resolve.ts` | 移植到 `frontend/src/theme/core/resolve.ts`；保留本地类型名与 Token 键 |
| `src/theme/derive/components.ts` | 移植到 `frontend/src/theme/derive/components.ts`；保留本地类型名与 Token 键 |
| `src/theme/types.ts` | 移植到 `frontend/src/theme/types.ts`；保留本地类型名与 Token 键 |

## 验证证据与限制

- 修复前：官方普通 premium 额度断言失败（core 仍为 81 而非 82）；材质尺寸回归失败
  （柔光 small radius 11px，而默认为 6px）。旧断言和旧实现问题均已保留原始日志。
- Core 新提示断言在旧出口下为 None；本地 helper 的 upstream-status 正反例在旧分类下失败。
- workspace/all-targets Clippy、Rust 格式检查通过；最后仅修改的 Protocol/OpenAI 测试
  再做对应包的 all-targets Clippy，通过。
- 后端 8 个受影响包批量覆盖 3,879 个用例（不重复计入启动用例派生子进程）。
  首轮有 6 项失败：2 项上述旧额度断言、3 项临时 PostgreSQL 停止导致的连接超时、
  1 项上述暂停时钟用例。修改后额度所在模块 37 项、停机所在 latency 模块 30 项通过；
  恢复隔离 PostgreSQL/Redis 后 3 项集成用例通过。停机用例另连续 20 次通过。
  其余首轮成功证据复用，不重复全量执行。
- 临时数据库曾因本机磁盘满退出；释放本工作区可重建编译缓存后恢复服务，
  未修改数据库测试条件，也未连接生产。启动预检 29 项通过。
- 前端批量 1,158 项中首次 1,156 项通过，2 项仍断言旧错误显示位置；同步新官方展示后，
  所属文件 12 项全部通过。ESLint、vue-tsc、Vite build、frozen-lockfile 安装均通过。
- CI 工作流、工具链和发布平台既有合同测试合计 18 项通过；本轮 Actions 只更新固定 SHA，
  没有修改任务结构。actionlint 本机未安装，未执行；GitHub 执行与镜像构建留待提交后 CI。
- 浏览器模拟 API 验证通过：四种代理提示只显示一次、原 POST prompt、明暗主题及
  1440/390/320px 布局、材质切换保留默认/自定义尺寸、主题预览和账号多分组不重叠。
- 隐私检查使用仓库外临时 index 核对待提交内容；真实 index 保持未暂存。
- 现有需下载内核并运行于隔离 Linux 网络命名空间的 Mihomo 用例保持原有 ignored，
  不计作本轮通过。
- 验证使用本地环回 HTTP/WS、隔离 PostgreSQL/Redis 和浏览器模拟 API；不代表真实外网代理、
  上游账号、Linux 镜像或生产已验收。发布 CI、正式镜像、标签及服务器切换本轮未执行。
