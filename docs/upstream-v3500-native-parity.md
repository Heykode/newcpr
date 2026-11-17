# v3.50.0 原生更新接入记录

本记录是固定基线的合入证据，共用规则仍为
[官方更新合并核对清单](upstream-integration-checklist.md)。历史版本记录不覆盖本次代码合同。

## 固定范围

| 基线 | 提交 |
| --- | --- |
| 上次官方 | `0f5bf03e13a43b34c43b9e3a6dee935ad43f6d2b` |
| 官方 v3.50.0 标签 | `68b297fc8888e359db01deb5cc59640739be5ce6` |
| 本次官方目标，含发布后额度校验与组件更新 | `cc9600467af55cbcf31a1fa128745c56f2c22e4c` |
| 官方 UI 旧版本 | `68458e62e7fb9fadbacea75ce48522f2ba85b8b2` |
| 官方 UI 新版本 | `c5e268e948ec74bf34ae7eaed9504af800c38ec3` |
| NewCPR 本地起点 | `ec5b7b14facbef8e8a4139e4886348ff65e96ecb` |

官方主仓库差异包含 20 个提交、124 个文件；UI 子模块差异包含 50 个文件。
采用其中已批准的原生作用代码及实际依赖，按下表接入本地结构，不将文件数量当成功能完成度。
本地起点包含八个重登维护提交，保留其历史和功能。本次没有读取或改变服务器部署状态，
不将该本地起点描述成线上版本；不包含提交、推送、标签或部署。

## 作用代码与调用链

下列路径均相对仓库根目录；官方与本地同名模块省略重复目录。

| 项目与官方来源 | 本地实际接入 | 行为与验证入口 |
| --- | --- | --- |
| WS 建连换号，`2abbfb1fac` | OpenAI `provider/failure.rs` → Core `error/mod.rs`、`engine/coordinator.rs` → selector | 瞬时未发送失败使用既有建连及换号预算。官方 `selection_retry::refused_connections_respect_the_same_rotation_limit_over_http_and_websocket` 覆盖 HTTP/WS、次数及严格子线程归属 |
| 不可达代理入口排除，`ceeb68ba82`、`b9ccc0b702` | `transport/diagnostics.rs`、WS handshake → `ProviderError::unreachable_proxy` → `AttemptContext` → OpenAI selector 与 Redis `capacity_wait.rs` 两处重扫；xAI session selector 消费同一事实 | 只排除已确认无法拨通的主机/端口；不把隧道或代理鉴权失败当成入口失效。`unreachable_proxy_is_skipped_across_rotations_and_reconsidered_on_the_next_request` 使用实际本地 TCP/HTTP/WS，确认下一请求重新尝试 |
| 原生续接长等待迁移，`f9e364860a` | OpenAI `continuation_recovery_error` → Core `RotateForClientReplay` → OpenAI `provider/mod.rs` 选号迁移 → 客户端完整重放 | 需要安全输出边界、超出等待上限、允许重试及换号预算；Provider 另核对持久亲和，宽松或无会话不迁移。无候选返回原失败。`native_capacity_above_wait_limit_rotates_before_requesting_client_replay` 与 `continuation_full_client_replay_selects_another_account` 验证预算、绑定及不向 B 发送 A 的增量 |
| 容量错误交付，`f9e364860a`、`1b96c775b2` | API `openai/error.rs` 及 Responses WS `protocol.rs`，现有 Chat/Responses HTTP/SSE 调用共享投影 | 外发容量错误清理两种 retry header；普通限流保留。保留原始诊断、关联 ID、其他字段及客户端重放投影。API error/capacity、Chat 交付与 WS forward 覆盖实际出口 |
| 无数字状态流内错误，`ce12ba3f43` | OpenAI `transport/canonical.rs` → failure 分类 → Core 重试决策 | `invalid_request_error` 按请求错误分类，容量码仍优先。`bare_sse_error_without_status_preserves_request_and_capacity_classification` 覆盖三种真实 SSE 形状 |
| 额度桶配置与预热，`5b7b88eda` | Core `account/quota_settings.rs` → Admin model/use case → API wire/presenter → Store rows/query/adapter → selector；OpenAI quota snapshot/observation → `bucket_warmup.rs` → warmup worker → 持久 CAS | `{limitPercent,warmup}` 替换旧映射；重置满五分钟后复查，正常请求即使显示 0% 也算使用。默认关闭，模型权限有效，同批桶只发一次；进度跨重启保留，结果不明不自动重放。官方桶预热正反例及真实 PostgreSQL CAS 用例覆盖 |
| 旧数据迁移 | 新增 `0096_account_quota_settings.sql` 与冻结条目 | 保留全部旧上限，将 warmup 初始化为 null；新增独立进度及 revision。`turn_state_upgrade` 覆盖旧 schema 升级，旧迁移字节不变 |
| 子路径，`7ee9ea8b63`、`e64fb2d955` | Vite build base、router/history、API constants、头像及导出地址、系统 SSE、PWA；Docker/Compose 构建参数与部署说明 | `/` 与 `/proxy/` 构建；页面刷新、鉴权恢复、脚本/字体/图标及 API 前缀。后端仍服务根路径，反代剥前缀并重写 Cookie Path、根相对 Location，包括 Realtime |
| 额度提交校验及共享 UI，`32b7b88eda`、`9d2aad95ec` | 本地 `Base*`、controls、theme → `QuotaSettings.vue` 和现有消费者 | 移植显式/表单/全局尺寸继承、validateEvent、错误态、选择限制、空态文字、表格行/分页及滚动尺寸；保留 NewCPR 样式、多分组换行和手机卡片。实际组件浏览器验证见下文 |
| 依赖与构建 | Rust manifest/lock、前端 package/lock、Docker Node 26.13.1、现有 CI 中 install-action | 采用固定目标中的相关依赖更新，保留本地裁剪后的依赖图和既有选择性 CI；不复制上游插件依赖或新增重复全量门禁 |

## 额外核对与必要适配

| 差异 | 处理及理由 |
| --- | --- |
| 旧容量错误健康分豁免 | 上次固定官方基线已将 `server_is_overloaded`、`slow_down`、`overloaded_error` 纳入 Smart 短期计分，本地仍提前排除。此次移除旧豁免并移植官方计分列表、Core `CapacityRejected` 反馈及 EWMA 系数 0.4（普通失败仍为 0.2）与对应正反例；不把容量错误写成额度耗尽或凭据失效。这是旧合入遗留差异，不冒充 v3.50.0 新功能 |
| 本次复验暴露的错误出口漏接 | WS `error_event` 原先只改错误码，遗漏 retry headers 清理；现调用官方完整容量投影。坏代理排除后严格子线程选号错误曾被本地 `retry_is_prohibited` 条件遮蔽；按官方恢复原失败，未开放额外重试。对应回归曾失败，修后复验 |
| 本地模块布局和接口名称 | 官方 quota、Admin 部分分文件，本地仍在既有模块；`account_rotations` 对应本地 `account_switches`，Store error 使用本地 `with_source`。xAI 通过原生 AttemptContext 读取代理排除事实，不引入官方插件 adapter |
| 本地账号出口和观测时间 | 预热调用现有 `backend_client`/`warmup_one`，保留账号出口租约、固定 IPv6、请求 profile 和真实捕获时间。正常凭据轮换保留同身份额度/预热进度并推进 revision；新身份授权、恢复等按现有身份合同清除旧进度 |
| Unauthorized 后账号池耗尽 | 保留既有本地最终错误映射：耗池返回本地选号错误，不将最后一个账号的 401 冒充客户端认证失败。只改变最终交付，不能给重试增加预算；与此次坏代理原错误保留分支分别处理 |
| 前端接口和布局 | 官方 UI 公共行为移植到本地 Base/cp 组件，保留 sm/md/lg、已有插槽和账号布局。没有 DatePicker 消费者，不新增闲置组件。官方 UI 子模块指针不覆盖本地实现 |
| 本地构建兼容 | Vue compiler 类型声明引用 `@babel/types`；显式加入开发依赖，使干净安装不依赖偶然 hoist。头像原有非 axios URL 也改为使用 API_BASE_URL，避免子路径漏接。公开默认配置使用 `.env.example`，Vite 保留 `/` 默认值；真实 `.env` 继续忽略，遵守既有隐私合同 |
| 过时测试与规范 | 容量 retry headers、预热移除 generate:false、凭据更新时间整行比较和 import.meta 测试夹具同步新合同。凭据测试明确核对新 warmup revision，保留旧身份/时间断言，不放宽结果凑绿。完整重放测试使用本地既有规范化消息形状，仍逐项断言全量输入及 previous_response_id 缺失；受影响 API/架构/spec 同步更新 |

## 明确排除

- 插件运行时、上游 API Key 账号及其专属 CPR 互联；混合文件中的公共原生改动已单独接入。
- PRISM、BPS/Excel 双凭据实验；现有 Excel、State、重登与账号出口功能保留。
- 官方发布版本文件、发行通道和上游 AI skills 整包替换；NewCPR 仍按自己的发布流程。
- 本次一次性移植、调试及预览脚本、原始日志和测试服务配置均在仓库外，不作为产品代码提交。

## 验证与边界

2026-11-17 完成本地验证。先运行受影响包完整回归，之后仅复验失败与后续变更影响的范围；
下列结果合并使用同轮有效证据，不表示每次修改都重复执行全量测试。

| 范围 | 实际结果 |
| --- | --- |
| Rust 静态检查 | workspace/all-targets 编译、最终 `cargo fmt --all --check`、workspace/all-targets/all-features Clippy `-D warnings` 通过 |
| 后端完整回归与修复复验 | Core、Admin、API、Store、OpenAI、xAI 六包完整回归已执行；发现的失败均完成归因和定向复验。Admin 的 9 个库测试、449 个集成测试及 xAI 396 个集成测试通过；其余具体修复见以下行 |
| Core 与请求恢复 | 最终协调层相关 140 项通过；计分变更后账号选择/Provider 反馈等 36 项通过。HTTP/WS 拒绝连接、请求内坏代理排除及下一请求恢复均通过 |
| API 交付 | 容量与错误相关 43 项复验通过，另有针对 HTTP/SSE/Chat/WS 的类别与等待头断言；普通错误保留等待头，容量错误清除，原始诊断保留 |
| OpenAI 预热与续接 | 预热、代理换号等 28 项通过；后续计分、完整重放和真实流使用证据等 16 项通过。完整重放包含额度耗尽、容量拒绝两条链；额度流覆盖失败、error、截断、畸形终态、成功及有无实际输出 |
| PostgreSQL/Redis | 隔离 PostgreSQL 16 与 Redis 实例；完整 Store 回归后，凭据时钟、预热 CAS、备份导出/恢复与 worker lease 等 14 项复验通过。最初两项失败因测试 URL 缺少密码；补齐隔离环境并使用匹配的 PostgreSQL 客户端后通过，没有修改业务断言 |
| 迁移与架构 | 93 条旧冻结记录原样保留，94 个迁移文件哈希匹配；仅追加 0096。旧上限迁移及其他旧字段保留用例通过。架构 14 项通过，并补上已移植额度使用证据测试的模块注册，未削弱文件布局门禁 |
| 前端 | 完整回归初次 1173 项中两个旧 import.meta 夹具失败，修后会话相关 13 项通过。最后头像依赖接入影响的账号 API/导入/批量编辑等 615 项全部通过。完整 lint、类型检查及后续所改文件 lint 通过；最终根路径和 `/proxy/` 生产构建通过 |
| 实际浏览器 | 合成账号数据下验证额度上限与必填模型校验、错误焦点、保存回显、尺寸继承、validateEvent、多选限制、表格行高/类名、单页隐藏和零高度滚动。桌面与实际 480 CSS 像素窄屏可操作；子路径产物的页面恢复、登录 API、脚本/字体/图标带前缀并成功加载 |
| 发行辅助 | toolchains：3 tests / 7 assertions；CI workflows：8 tests / 186 assertions，通过 |
| 隐私与变更范围 | 使用独立临时 index 检查最终工作树内容，通过；没有改变用户暂存区、隐私策略或豁免清单。`git diff --check` 通过；一次性工具未进入源码 |

静态检查发现既有重登维护测试四处不必要的 slice clone，改用 `std::slice::from_ref`；
不改变其业务逻辑、断言或八个已有提交。前端 API 测试补齐新增常量依赖，另验证头像 URL
在根路径和子路径下的编码；不为旧 CommonJS 夹具删除生产的 Vite base 支持。

未执行的边界：没有真实上游账号请求、服务器测试或部署；没有复跑 Linux CI 或 PostgreSQL 18。
TLS ClientHello 对比用例因没有指定官方 Codex 参考可执行文件保持原有 ignored 状态。
浏览器使用合成接口，不能代替真实反代下 Cookie/Realtime 的部署验收；Redis 数据层和生产
选号等待代码分别验证，不能将测试替身称作完整真实 Redis 代理故障演练。
macOS 调试链接器提示大型 unwind 表，构建与测试完成；未通过关闭警告来掩盖代码问题。
未运行发布历史隐私扫描；提交/推送时仍须检查届时精确暂存内容及完整发布历史。
