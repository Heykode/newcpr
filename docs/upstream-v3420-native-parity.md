# 官方 v3.42.0 原生更新对齐记录

目标仓库 `Heykode/newcpr`，本地起点 `e912a343d6b7667677897fd85c99fbbd799514f6`。
官方范围固定为 `1240f22580eaa8b4009f6ca3e9b2a15c07916d5f` →
`81de0f533136697d52565d842ccb09cc3baa1759`（2026-11-07），共19提交、165条最终增量路径。
以最终源码和调用链移植；两仓历史不直接合并，不以描述相近替代代码核对。

## 功能与接入差异

| 范围 | 本轮结果及必要差异 |
| --- | --- |
| Key/账号排队 | 官方独立时限、共享账号预算、队满/超时一次立即重选。默认不开启重选；严格普通根取得新账号后 CAS 迁移绑定，Preferred 保留原软绑定，fixed/native/严格后代不跨号。OpenAI 保留 Redis owner，xAI 采用官方进程内队列并通过现有 Store 准入。 |
| Credits | 按账号显式开启、默认关闭；新鲜点数证据放行，关闭撤销 credit-only 状态并拦截迟到写。关闭时不触发失败补查；开启时后台补查不重放已失败请求。OAuth 成功不独立恢复额度。本地失败观察的 PreserveAccess 和独立观察时钟保留。 |
| 安装种子 | 官方 SHA-256 域分隔和长度前缀算法，种子+Key ID+账号 ID派生 UUID；HTTP/WS、目录缓存、Live 全链路接入。无种子沿用账号设备；旧配置迁移后有效出站身份会改变，账号设备记录不改。 |
| OAuth 参数 | 官方移除不支持的顶层字段、stream_options.include_usage，规范 token 上限别名；保留有效的显式截断/停止/缓存选项及嵌套业务字段。仅 OAuth 原生链路调用；Excel 共用的历史归一化不包含该步骤。 |
| 更新/发布 | 最终 update-index→已发布 version.yaml→精确 tag；有界缓存、冷却/重试时间、通道冻结、版本只读和 checksum先取。发布 prepare→publish→finish及 index_only 恢复接入现有签名/CI证据链，单体安装与回滚仍由原 owner 负责。 |
| 管理界面 | 点数开关、安装种子表单、更新通道/错误重试、按钮复用、组名 Popover、时间单位。采用现有 Base 组件、样式及 API 封装，不引入第二套 UI 库。 |
| 数据库 | 追加0086，保留0085及以前迁移字节。旧队列字段改新 canonical 名，已有超时归入1..120秒，人数/显式关闭/其他参数保留；新建库沿已有0083默认，旧客户端缺字段兼容默认仍保持独立。 |

继续排除已确定不接入的插件运行时、peer_link、上游 API-key 产品和已取消功能。
官方混合文件中可独立迁入的原生行为单独移植。版本号、正式发行说明不随官方替换；本轮不执行生产部署或正式发版。

两处既有接入差异继续明确保留：诊断探测走独立观察 owner，不在诊断错误后另起 usage 补查，
普通请求按官方点数开关执行补查；Live 的 session/thread 仍按不同 namespace 派生关联 ID，
本轮官方增量未修改关联算法。移植回归分别冻结并核对两者从 bootstrap 到 sideband/hangup
的一致性，完整保留官方新增的种子不泄漏、安装身份冻结和凭据/出口热更新检查。

## 合并时发现并修正

- 旧额度错误处理漏了官方 Credits 开关判断；采用官方函数，关闭不再安排 usage 补查。
- OAuth 参数清理最初接在 Excel 也使用的共享历史函数；移到原生 OAuth 入口，保留 Excel 合同。
- Credits 新增第二根节点后父级宽度 attrs 无法继承；显式绑定主容器，恢复移动端可用宽度。
- 历史更新失败与当前任务状态分开，旧失败不会永久阻止再次更新。
- 旧设置上限、旧更新列表查询、旧 OAuth 透传和成功即恢复额度的测试按官方新合同修正；旧点数补查重复用例由官方开关正反例覆盖。
- macOS 在取消积压 WebSocket 后可能以 TCP ConnectionReset结束；用例接受此明确清理结果，仍保留取消到达超时及其他错误检查，未改变生产传输行为。
- 架构检查同步官方新增的公共 concurrency owner：仅允许 account/selection 引用它，继续禁止反向依赖 engine/routing；未为旧模块依赖清单撤销新的独立排队预算。
- CI发现旧0069历史回填测试把账号整行不变断言延伸到0086，误拒绝合法新增的点数开关。固定0069和0083各自的迁移验证终点；跨版本Excel/State升级测试显式检查新开关默认false后继续逐字段比较原账号，不修改生产实现或已发布迁移。

## 提交对应

| 官方提交 | 内容 | 本地处理 |
| --- | --- | --- |
| `ee7e837d` | feat(scheduling): split queue timeouts and allow account fallback (#625) | 原生移植，适配现有 Redis/Store owner |
| `e907f419` | fix(ui): show account group names in popovers | 现有完整名 Popover保持并补齐显示 |
| `b7fa5872` | refactor(ui): reuse shared button components | 复用本地 Base组件 |
| `2a8d5d86` | ci(release): publish the channel index after verifying release assets | 采用最终快照发布算法 |
| `4326eb1e` | fix(update): bound GitHub lookups with a published channel index (#627) | 采用最终发现算法 |
| `90ecac97` | feat(accounts): make quota credit use opt-in per account (#628) | 全链路接入与迁移 |
| `e97b1faf` | feat(openai): derive installation identities from key seeds (#629) | 身份派生及所有调用入口接入 |
| `0d25061e` | docs: bound UI verification and generalize AI attribution | 按影响验收、停止重复；保留实际署名 |
| `228eea90` | chore(db): consolidate unreleased migrations | 新增0086，不改本地冻结旧迁移 |
| `d03d8408` | docs: clarify current configuration and update behavior | 同步本地 API、架构和部署说明 |
| `55526973` | fix(openai): normalize unsupported Codex OAuth request options (#630) | 原生 OAuth入口接入 |
| `e275a6da` | chore(release): v3.42.0 | 不替换本地版本与发行说明 |
| `56ef1bff` | test(openai): align service tier fixture with OAuth normalization | OAuth相关夹具同步 |
| `ac98a764` | fix(update): discover channels from verified release assets | 采用该最终实现，替代较早索引方案 |
| `7cee4309` | chore(release): v3.42.0 | 不替换本地版本与发行说明 |
| `d0a588fb` | test(plugin): signal crashes after host registration | 插件测试排除 |
| `ba87cfde` | chore(ci): remove the release script test suite | 本地不同发布链仍需其必要回归，不新增官方已删套件 |
| `69c1ee15` | docs(release): clarify same-version retries and simplify release notes | 采纳快照恢复；不自动清理已发布 tag |
| `81de0f53` | build(release): keep the plugin manifest inside distribution packages | 仅插件发行物排除 |

## 165路径差异账

“原样”指最终文件与固定官方源码相同；“接入”指官方增量落入现有本地 owner/目录，
不表示整份文件相同。“保留/排除”给出逐项理由；共享类型的新增字段同步所有本地构造器。

| # | 官方增量路径 | 处理/本地落点 | 说明 |
| --- | --- | --- | --- |
| 1 | `.agents/skills/cpr-dev-guide/SKILL.md` | 接入：`docs/upstream-integration-checklist.md` | 按影响范围验收、复用有效证据 |
| 2 | `.agents/skills/cpr-dev-guide/references/frontend.md` | 接入：`docs/upstream-integration-checklist.md` | 同上；相关页面完成后停止重复验证 |
| 3 | `.agents/skills/cpr-release/SKILL.md` | 接入：`deploy/README.md` | 通道快照与公开 tag 恢复边界 |
| 4 | `.agents/skills/cpr-release/references/experimental.md` | 接入：`deploy/README.md` | 实验通道按基线隔离 |
| 5 | `.agents/skills/cpr-release/references/operations.md` | 接入：`deploy/README.md` | 仅合入索引恢复规则，不自动重写版本历史 |
| 6 | `.agents/skills/cpr-release/references/preparation.md` | 接入：`deploy/README.md` | 构建版本与已发布通道快照分离 |
| 7 | `.github/scripts/package-release.rb` | 保留/排除 | 仅官方插件 manifest 打包；本项目不包含官方插件发行物 |
| 8 | `.github/scripts/published-channels.rb` | 原样：`release/published-channels.rb` | 发布/流程：沿本地 owner 同步字段、行为或相关回归 |
| 9 | `.github/scripts/release-checks-test.rb` | 保留/排除 | 保留本地发布预检/证据复用回归；并非被删除的官方插件脚本套件 |
| 10 | `.github/scripts/release-metadata.rb` | 接入：`release/release-metadata.rb` | 发布/流程：沿本地 owner 同步字段、行为或相关回归 |
| 11 | `.github/workflows/_quality.yml` | 保留/排除 | 同上；本轮不取消现有质量门禁 |
| 12 | `.github/workflows/release.yml` | 接入：`.github/workflows/release.yml` | 发布/流程：沿本地 owner 同步字段、行为或相关回归 |
| 13 | `CONTRIBUTING.md` | 接入：`docs/upstream-integration-checklist.md` | 保留实际 Codex 署名；本地协作入口不引入官方缺失技能 |
| 14 | `backend/Cargo.lock` | 接入：`backend/Cargo.lock` | 仅新增 gateway-host 的 yaml-rust2 依赖边，复用锁定版本 |
| 15 | `backend/crates/gateway-admin/src/model/accounts.rs` | 接入：`backend/crates/gateway-admin/src/model/accounts.rs` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 16 | `backend/crates/gateway-admin/src/model/settings.rs` | 接入：`backend/crates/gateway-admin/src/model/settings.rs` | 排队/设置：沿本地 owner 同步字段、行为或相关回归 |
| 17 | `backend/crates/gateway-admin/src/model/system.rs` | 接入：`backend/crates/gateway-admin/src/model/system.rs` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 18 | `backend/crates/gateway-admin/src/ports/system.rs` | 接入：`backend/crates/gateway-admin/src/ports/system.rs` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 19 | `backend/crates/gateway-admin/src/use_case/accounts/mod.rs` | 接入：`backend/crates/gateway-admin/src/use_case/accounts.rs` | 沿现有账号能力和异步额度观察端口接入 |
| 20 | `backend/crates/gateway-admin/src/use_case/credential_mutation.rs` | 接入：`backend/crates/gateway-admin/src/use_case/openai.rs` | 沿既有配置发布与后台观察函数拆分 |
| 21 | `backend/crates/gateway-admin/src/use_case/system.rs` | 接入：`backend/crates/gateway-admin/src/use_case/system.rs` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 22 | `backend/crates/gateway-admin/tests/use_case/accounts/mod.rs` | 接入：`backend/crates/gateway-admin/tests/use_case/accounts.rs` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 23 | `backend/crates/gateway-admin/tests/use_case/freeze_recovery.rs` | 保留/排除 | 本地冻结恢复测试不构造官方 SettingsValues；共享设置已统一适配 |
| 24 | `backend/crates/gateway-admin/tests/use_case/mod.rs` | 接入：`backend/crates/gateway-admin/tests/use_case/mod.rs` | 界面/夹具：沿本地 owner 同步字段、行为或相关回归 |
| 25 | `backend/crates/gateway-admin/tests/use_case/observability/mod.rs` | 接入：`backend/crates/gateway-admin/tests/use_case/mod.rs` | 界面/夹具：沿本地 owner 同步字段、行为或相关回归 |
| 26 | `backend/crates/gateway-admin/tests/use_case/settings.rs` | 接入：`backend/crates/gateway-admin/tests/use_case/settings.rs` | 排队/设置：沿本地 owner 同步字段、行为或相关回归 |
| 27 | `backend/crates/gateway-admin/tests/use_case/system.rs` | 接入：`backend/crates/gateway-admin/tests/use_case/system.rs` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 28 | `backend/crates/gateway-admin/tests/use_case/system_preflight.rs` | 保留/排除 | 本地无独立 system_preflight 测试入口；更新操作端口及 Host 回归已适配 |
| 29 | `backend/crates/gateway-api/src/admin/accounts/presenter.rs` | 接入：`backend/crates/gateway-api/src/admin/accounts/presenter.rs` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 30 | `backend/crates/gateway-api/src/admin/accounts/wire.rs` | 接入：`backend/crates/gateway-api/src/admin/accounts/wire.rs` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 31 | `backend/crates/gateway-api/src/admin/settings.rs` | 接入：`backend/crates/gateway-admin/src/model/settings.rs` | 本地 API 直接展平 RequestTuningOverrides；使用 canonical 名及旧名 alias |
| 32 | `backend/crates/gateway-api/src/admin/system.rs` | 接入：`backend/crates/gateway-api/src/admin/system.rs` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 33 | `backend/crates/gateway-api/tests/admin/accounts/handlers.rs` | 接入：`backend/crates/gateway-api/tests/admin/accounts/mod.rs` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 34 | `backend/crates/gateway-api/tests/admin/errors.rs` | 接入：`backend/crates/gateway-api/tests/admin/errors.rs` | 界面/夹具：沿本地 owner 同步字段、行为或相关回归 |
| 35 | `backend/crates/gateway-api/tests/admin/mod.rs` | 接入：`backend/crates/gateway-api/tests/admin/mod.rs` | 界面/夹具：沿本地 owner 同步字段、行为或相关回归 |
| 36 | `backend/crates/gateway-api/tests/admin/settings.rs` | 接入：`backend/crates/gateway-api/tests/admin/settings.rs` | 排队/设置：沿本地 owner 同步字段、行为或相关回归 |
| 37 | `backend/crates/gateway-api/tests/admin/system.rs` | 接入：`backend/crates/gateway-api/tests/admin/system.rs` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 38 | `backend/crates/gateway-api/tests/openai/models.rs` | 保留/排除 | 插件中间件设置测试，原生目录入口另在 Provider/Core 接入 |
| 39 | `backend/crates/gateway-api/tests/openai/peer_link/mod.rs` | 保留/排除 | peer_link 已排除 |
| 40 | `backend/crates/gateway-api/tests/support.rs` | 接入：`backend/crates/gateway-api/tests/admin/mod.rs` | 界面/夹具：沿本地 owner 同步字段、行为或相关回归 |
| 41 | `backend/crates/gateway-core/src/account/model.rs` | 接入：`backend/crates/gateway-core/src/account/model.rs` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 42 | `backend/crates/gateway-core/src/account/selection.rs` | 接入：`backend/crates/gateway-core/src/account/selection.rs` | 排队/设置：沿本地 owner 同步字段、行为或相关回归 |
| 43 | `backend/crates/gateway-core/src/concurrency.rs` | 接入：`backend/crates/gateway-core/src/concurrency.rs` | 排队/设置：沿本地 owner 同步字段、行为或相关回归 |
| 44 | `backend/crates/gateway-core/src/engine/coordinator.rs` | 接入：`backend/crates/gateway-core/src/engine/execution.rs` | Key 准入不再向账号传递已经消耗的排队预算 |
| 45 | `backend/crates/gateway-core/src/engine/execution/scope.rs` | 接入：`backend/crates/gateway-core/src/engine/execution.rs` | 同上，沿本地 Execution owner 接入 |
| 46 | `backend/crates/gateway-core/src/engine/execution/service.rs` | 接入：`backend/crates/gateway-core/src/engine/execution.rs` | 独立预算和模型目录 Key ID 传递 |
| 47 | `backend/crates/gateway-core/src/engine/mod.rs` | 接入：`backend/crates/gateway-core/src/engine/capacity_wait.rs` | 本地已有独立 ConcurrencyWaitContext owner |
| 48 | `backend/crates/gateway-core/src/engine/provider.rs` | 接入：`backend/crates/gateway-core/src/engine/provider.rs` | 安装种子：沿本地 owner 同步字段、行为或相关回归 |
| 49 | `backend/crates/gateway-core/src/settings/compiled.rs` | 接入：`backend/crates/gateway-core/src/routing/snapshot.rs` | 编译独立时间及重选设置进入冻结快照 |
| 50 | `backend/crates/gateway-core/src/settings/values.rs` | 接入：`backend/crates/gateway-admin/src/model/settings.rs` | 保留内部字段名，外部使用官方新名；范围 1..120 |
| 51 | `backend/crates/gateway-core/tests/concurrency.rs` | 原样：`backend/crates/gateway-core/tests/concurrency.rs` | 排队/设置：沿本地 owner 同步字段、行为或相关回归 |
| 52 | `backend/crates/gateway-core/tests/engine/execution/mod.rs` | 接入：`backend/crates/gateway-core/tests/engine/execution.rs` | 排队/设置：沿本地 owner 同步字段、行为或相关回归 |
| 53 | `backend/crates/gateway-host/Cargo.toml` | 接入：`backend/crates/gateway-host/Cargo.toml` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 54 | `backend/crates/gateway-host/src/system_update/discovery.rs` | 原样：`backend/crates/gateway-host/src/system_update/discovery.rs` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 55 | `backend/crates/gateway-host/src/system_update/download.rs` | 接入：`backend/crates/gateway-host/src/system_update/download.rs` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 56 | `backend/crates/gateway-host/src/system_update/metadata.rs` | 原样：`backend/crates/gateway-host/src/system_update/metadata.rs` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 57 | `backend/crates/gateway-host/src/system_update/mod.rs` | 接入：`backend/crates/gateway-host/src/system_update/mod.rs` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 58 | `backend/crates/gateway-host/src/system_update/operations.rs` | 接入：`backend/crates/gateway-host/src/system_update/mod.rs` | 发现/下载按官方；单体替换、回滚与重启沿本地 owner |
| 59 | `backend/crates/gateway-host/src/system_update/release.rs` | 原样：`backend/crates/gateway-host/src/system_update/release.rs` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 60 | `backend/crates/gateway-host/tests/system_update/channels.rs` | 接入：`backend/crates/gateway-host/tests/system_update/channels.rs` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 61 | `backend/crates/gateway-host/tests/system_update/discovery.rs` | 接入：`backend/crates/gateway-host/tests/system_update/discovery.rs` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 62 | `backend/crates/gateway-host/tests/system_update/download.rs` | 接入：`backend/crates/gateway-host/tests/system_update/mod.rs` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 63 | `backend/crates/gateway-host/tests/system_update/installation.rs` | 接入：`backend/crates/gateway-host/tests/system_update/mod.rs` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 64 | `backend/crates/gateway-host/tests/system_update/mod.rs` | 接入：`backend/crates/gateway-host/tests/system_update/mod.rs` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 65 | `backend/crates/gateway-host/tests/system_update/operations.rs` | 接入：`backend/crates/gateway-host/tests/system_update/mod.rs` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 66 | `backend/crates/gateway-host/tests/system_update/release.rs` | 接入：`backend/crates/gateway-host/tests/system_update/mod.rs` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 67 | `backend/crates/gateway-plugin/runtime/tests/generation/data.rs` | 保留/排除 | 插件运行时已排除 |
| 68 | `backend/crates/gateway-plugin/runtime/tests/generation/diagnostics.rs` | 保留/排除 | 插件运行时已排除 |
| 69 | `backend/crates/gateway-plugin/runtime/tests/generation/instance.rs` | 保留/排除 | 插件运行时已排除 |
| 70 | `backend/crates/gateway-plugin/runtime/tests/support/environment.rs` | 保留/排除 | 插件运行时已排除 |
| 71 | `backend/crates/gateway-plugin/runtime/tests/support/worker/mod.rs` | 保留/排除 | 插件运行时已排除 |
| 72 | `backend/crates/gateway-plugin/sdk/src/call/services/settings.rs` | 保留/排除 | 插件 SDK 已排除；原生设置合同已合入 |
| 73 | `backend/crates/gateway-store/src/admin_adapter.rs` | 接入：`backend/crates/gateway-store/src/admin_adapter.rs` | 本地审计整体记录 request_tuning；新字段随该合同流转，无需重复审计清单 |
| 74 | `backend/crates/gateway-store/src/postgres/account_groups.rs` | 接入：`backend/crates/gateway-store/src/postgres/account_groups/mod.rs` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 75 | `backend/crates/gateway-store/src/postgres/provider_accounts/admin_adapter.rs` | 接入：`backend/crates/gateway-store/src/postgres/provider_accounts/admin_adapter.rs` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 76 | `backend/crates/gateway-store/src/postgres/provider_accounts/admin_queries.rs` | 接入：`backend/crates/gateway-store/src/postgres/provider_accounts/admin_queries.rs` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 77 | `backend/crates/gateway-store/src/postgres/provider_accounts/mapping.rs` | 接入：`backend/crates/gateway-store/src/postgres/provider_accounts/mapping.rs` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 78 | `backend/crates/gateway-store/src/postgres/provider_accounts/repository.rs` | 接入：`backend/crates/gateway-store/src/postgres/provider_accounts/repository.rs` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 79 | `backend/crates/gateway-store/src/postgres/provider_accounts/rows.rs` | 接入：`backend/crates/gateway-store/src/postgres/provider_accounts/rows.rs` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 80 | `backend/crates/gateway-store/src/postgres/runtime_settings.rs` | 接入：`backend/crates/gateway-admin/src/model/settings.rs` | 本地队列配置存 request_tuning_json；序列化 owner 是 Admin 模型 |
| 81 | `backend/crates/gateway-store/src/postgres/snapshot.rs` | 接入：`backend/crates/gateway-store/src/postgres/snapshot.rs` | 安装种子：沿本地 owner 同步字段、行为或相关回归 |
| 82 | `backend/crates/gateway-store/tests/postgres/provider_accounts/mod.rs` | 接入：`backend/crates/gateway-store/tests/postgres/provider_accounts/mod.rs` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 83 | `backend/crates/gateway-store/tests/postgres/proxies.rs` | 接入：`backend/crates/gateway-store/tests/postgres/proxies.rs` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 84 | `backend/crates/gateway-store/tests/postgres/runtime_settings.rs` | 接入：`backend/crates/gateway-store/tests/postgres/runtime_settings.rs` | 排队/设置：沿本地 owner 同步字段、行为或相关回归 |
| 85 | `backend/crates/providers/openai/src/admin/mod.rs` | 接入：`backend/crates/providers/openai/src/admin.rs` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 86 | `backend/crates/providers/openai/src/compat/request.rs` | 接入：`backend/crates/providers/openai/src/compat/request.rs` | 协议/额度接入：沿本地 owner 同步字段、行为或相关回归 |
| 87 | `backend/crates/providers/openai/src/credential/catalog.rs` | 接入：`backend/crates/providers/openai/src/credential/catalog.rs` | 安装种子：沿本地 owner 同步字段、行为或相关回归 |
| 88 | `backend/crates/providers/openai/src/credential/quota/passive.rs` | 接入：`backend/crates/providers/openai/src/credential/quota/mod.rs` | 官方额度判定接入本地捕获时钟及 PreserveAccess；不恢复旧成功放行 |
| 89 | `backend/crates/providers/openai/src/credential/quota/recovery.rs` | 接入：`backend/crates/providers/openai/src/credential/quota/recovery.rs` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 90 | `backend/crates/providers/openai/src/credential/quota/service.rs` | 接入：`backend/crates/providers/openai/src/credential/quota/mod.rs` | 官方成功/点数/主动恢复逻辑，保留独立访问观察时间保护 |
| 91 | `backend/crates/providers/openai/src/credential/quota/snapshot.rs` | 接入：`backend/crates/providers/openai/src/credential/quota/snapshot.rs` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 92 | `backend/crates/providers/openai/src/credential/quota/warmup.rs` | 接入：`backend/crates/providers/openai/src/credential/quota/mod.rs` | 耗尽账号跳过 warmup |
| 93 | `backend/crates/providers/openai/src/credential/selector.rs` | 接入：`backend/crates/providers/openai/src/credential/selector/mod.rs` | 官方一次立即重选/绑定规则接入 Redis 队位、租约、CAS owner |
| 94 | `backend/crates/providers/openai/src/provider/execution.rs` | 接入：`backend/crates/providers/openai/src/provider/execution.rs` | 协议/额度接入：沿本地 owner 同步字段、行为或相关回归 |
| 95 | `backend/crates/providers/openai/src/provider/failure.rs` | 接入：`backend/crates/providers/openai/src/provider/failure.rs` | 协议/额度接入：沿本地 owner 同步字段、行为或相关回归 |
| 96 | `backend/crates/providers/openai/src/provider/live.rs` | 接入：`backend/crates/providers/openai/src/provider/live.rs` | bootstrap、sideband、hangup 共用冻结种子画像 |
| 97 | `backend/crates/providers/openai/src/provider/mod.rs` | 接入：`backend/crates/providers/openai/src/provider/mod.rs` | 协议/额度接入：沿本地 owner 同步字段、行为或相关回归 |
| 98 | `backend/crates/providers/openai/src/transport/client_catalog.rs` | 接入：`backend/crates/providers/openai/src/transport/catalog.rs` | 本地 transport/catalog.rs 已直接消费 context，无 Key installation_id 覆盖 |
| 99 | `backend/crates/providers/openai/src/transport/client_live.rs` | 接入：`backend/crates/providers/openai/src/transport/client_live.rs` | 安装种子：沿本地 owner 同步字段、行为或相关回归 |
| 100 | `backend/crates/providers/openai/src/transport/profile/identity.rs` | 接入：`backend/crates/providers/openai/src/transport/profile/identity.rs` | 安装种子：沿本地 owner 同步字段、行为或相关回归 |
| 101 | `backend/crates/providers/openai/src/transport/profile/mod.rs` | 接入：`backend/crates/providers/openai/src/transport/profile/mod.rs` | 安装种子：沿本地 owner 同步字段、行为或相关回归 |
| 102 | `backend/crates/providers/openai/src/transport/profile/selection.rs` | 接入：`backend/crates/providers/openai/src/transport/profile/selection.rs` | 安装种子：沿本地 owner 同步字段、行为或相关回归 |
| 103 | `backend/crates/providers/openai/tests/admin/mod.rs` | 接入：`backend/crates/providers/openai/tests/admin.rs` | 安装种子：沿本地 owner 同步字段、行为或相关回归 |
| 104 | `backend/crates/providers/openai/tests/credential/catalog.rs` | 接入：`backend/crates/providers/openai/tests/credential/catalog.rs` | 安装种子：沿本地 owner 同步字段、行为或相关回归 |
| 105 | `backend/crates/providers/openai/tests/credential/contract/mod.rs` | 接入：`backend/crates/providers/openai/tests/credential/contract/mod.rs` | 排队/设置：沿本地 owner 同步字段、行为或相关回归 |
| 106 | `backend/crates/providers/openai/tests/credential/contract/scheduling_projection.rs` | 接入：`backend/crates/providers/openai/tests/credential/contract/scheduling_projection.rs` | 排队/设置：沿本地 owner 同步字段、行为或相关回归 |
| 107 | `backend/crates/providers/openai/tests/credential/profile_statistics.rs` | 接入：`backend/crates/providers/openai/tests/credential/profile_statistics.rs` | 安装种子：沿本地 owner 同步字段、行为或相关回归 |
| 108 | `backend/crates/providers/openai/tests/credential/quota/credits.rs` | 接入：`backend/crates/providers/openai/tests/credential/quota/credits.rs` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 109 | `backend/crates/providers/openai/tests/credential/quota/mod.rs` | 接入：`backend/crates/providers/openai/tests/credential/quota/mod.rs` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 110 | `backend/crates/providers/openai/tests/provider/contract/capacity.rs` | 接入：`backend/crates/providers/openai/tests/credential/contract/scheduling_projection.rs` | 严格队满/超时、Preferred/sessionless 无延长重排队回归 |
| 111 | `backend/crates/providers/openai/tests/provider/contract/mod.rs` | 接入：`backend/crates/providers/openai/tests/provider/contract/mod.rs` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 112 | `backend/crates/providers/openai/tests/provider/contract/passthrough.rs` | 接入：`backend/crates/providers/openai/tests/provider/contract/generate_compat.rs` | 官方 OAuth 正反例；排除上游 API-key 产品分支，Excel 独立调用 |
| 113 | `backend/crates/providers/openai/tests/provider/live.rs` | 接入：`backend/crates/providers/openai/tests/provider/live.rs` | 官方 Linux TLS 回环身份冻结用例接入本地 Store 与 Arc 画像 |
| 114 | `backend/crates/providers/openai/tests/support.rs` | 接入：`backend/crates/providers/openai/tests/support.rs` | 安装种子：沿本地 owner 同步字段、行为或相关回归 |
| 115 | `backend/crates/providers/openai/tests/transport/catalog.rs` | 接入：`backend/crates/providers/openai/tests/transport/catalog.rs` | 安装种子：沿本地 owner 同步字段、行为或相关回归 |
| 116 | `backend/crates/providers/openai/tests/transport/mod.rs` | 接入：`backend/crates/providers/openai/tests/transport/mod.rs` | 安装种子：沿本地 owner 同步字段、行为或相关回归 |
| 117 | `backend/crates/providers/openai/tests/transport/profile/mod.rs` | 接入：`backend/crates/providers/openai/tests/transport/profile/mod.rs` | 安装种子：沿本地 owner 同步字段、行为或相关回归 |
| 118 | `backend/crates/providers/openai/tests/transport/reset_credits.rs` | 接入：`backend/crates/providers/openai/tests/transport/reset_credits.rs` | 安装种子：沿本地 owner 同步字段、行为或相关回归 |
| 119 | `backend/crates/providers/openai/tests/transport/usage.rs` | 接入：`backend/crates/providers/openai/tests/transport/usage.rs` | 安装种子：沿本地 owner 同步字段、行为或相关回归 |
| 120 | `backend/crates/providers/xai/src/credential/selector.rs` | 接入：`backend/crates/providers/xai/src/credential/selector.rs` | 官方进程内队列算法接入现有 xAI Store 准入和错误映射 |
| 121 | `backend/crates/providers/xai/tests/admin.rs` | 接入：`backend/crates/providers/xai/tests/admin.rs` | 界面/夹具：沿本地 owner 同步字段、行为或相关回归 |
| 122 | `backend/crates/providers/xai/tests/credential/selector.rs` | 接入：`backend/crates/providers/xai/tests/credential/selector.rs` | 排队/设置：沿本地 owner 同步字段、行为或相关回归 |
| 123 | `backend/migrations/.frozen-sha256` | 接入：`backend/migrations/.frozen-sha256` | 只追加0086哈希；已发布迁移字节不改 |
| 124 | `backend/migrations/0039_queue_timeouts_quota_credits_and_installation_seed.sql` | 接入：`backend/migrations/0086_quota_credits_and_installation_seed.sql` | 适配现有表与 JSON：点数开关/迟到写保护/种子转换/超时范围收敛 |
| 125 | `deploy/README.md` | 接入：`deploy/README.md` | 迁移/说明：沿本地 owner 同步字段、行为或相关回归 |
| 126 | `docs/api.md` | 接入：`docs/api.md` | 迁移/说明：沿本地 owner 同步字段、行为或相关回归 |
| 127 | `docs/architecture.md` | 接入：`docs/architecture.md` | 更新本地 owner 架构说明 |
| 128 | `frontend/src/api/modules/accounts/management.ts` | 接入：`frontend/src/api/modules/accounts.ts` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 129 | `frontend/src/api/modules/accounts/types.ts` | 接入：`frontend/src/api/modules/accounts.ts` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 130 | `frontend/src/api/modules/settings/index.ts` | 接入：`frontend/src/api/modules/settings.ts` | 排队/设置：沿本地 owner 同步字段、行为或相关回归 |
| 131 | `frontend/src/api/modules/settings/profiles.ts` | 接入：`frontend/src/api/modules/settings.ts` | 安装种子：沿本地 owner 同步字段、行为或相关回归 |
| 132 | `frontend/src/api/modules/settings/system.ts` | 接入：`frontend/src/api/modules/system.ts` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 133 | `frontend/src/components/AccountGroupMarks.vue` | 接入：`frontend/src/components/AccountGroupMarks.vue` | 保留已有完整组名 Popover，实现同等官方可见行为 |
| 134 | `frontend/src/components/usage/UsageDetailPopover.vue` | 接入：`frontend/src/views/usage/components/UsageDetailPopover.vue` | 界面/夹具：沿本地 owner 同步字段、行为或相关回归 |
| 135 | `frontend/src/i18n/locales/en/accounts.json` | 接入：`frontend/src/i18n/locales/en/accounts.json` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 136 | `frontend/src/i18n/locales/en/settings.json` | 接入：`frontend/src/i18n/locales/en/settings.json` | 界面/夹具：沿本地 owner 同步字段、行为或相关回归 |
| 137 | `frontend/src/i18n/locales/en/systemUpdate.json` | 接入：`frontend/src/i18n/locales/en/systemUpdate.json` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 138 | `frontend/src/i18n/locales/zh-Hans/accounts.json` | 接入：`frontend/src/i18n/locales/zh-Hans/accounts.json` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 139 | `frontend/src/i18n/locales/zh-Hans/settings.json` | 接入：`frontend/src/i18n/locales/zh-Hans/settings.json` | 界面/夹具：沿本地 owner 同步字段、行为或相关回归 |
| 140 | `frontend/src/i18n/locales/zh-Hans/systemUpdate.json` | 接入：`frontend/src/i18n/locales/zh-Hans/systemUpdate.json` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 141 | `frontend/src/i18n/locales/zh-Hant/accounts.json` | 接入：`frontend/src/i18n/locales/zh-Hant/accounts.json` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 142 | `frontend/src/i18n/locales/zh-Hant/settings.json` | 接入：`frontend/src/i18n/locales/zh-Hant/settings.json` | 界面/夹具：沿本地 owner 同步字段、行为或相关回归 |
| 143 | `frontend/src/i18n/locales/zh-Hant/systemUpdate.json` | 接入：`frontend/src/i18n/locales/zh-Hant/systemUpdate.json` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 144 | `frontend/src/layout/components/system-update-modal/index.vue` | 接入：`frontend/src/layout/components/SystemUpdateModal/index.vue` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 145 | `frontend/src/layout/components/system-update-modal/presenter.ts` | 接入：`frontend/src/layout/components/SystemUpdateModal/presenter.ts` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 146 | `frontend/src/layout/index.vue` | 接入：`frontend/src/layout/index.vue` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 147 | `frontend/src/stores/modules/system-update.ts` | 接入：`frontend/src/stores/modules/system-update.ts` | 更新发现：沿本地 owner 同步字段、行为或相关回归 |
| 148 | `frontend/src/views/accounts/components/account-quota-panel/Credits.vue` | 接入：`frontend/src/views/accounts/components/AccountQuotaPanel/Credits.vue` | 接入已有请求封装与 BaseSwitch；保留移动宽度 attrs |
| 149 | `frontend/src/views/accounts/components/account-quota-panel/ResetCreditAutomation.vue` | 保留/排除 | 本地无自动使用重置卡组件；本次仅修改该组件文案与 tooltip，不新增产品 |
| 150 | `frontend/src/views/accounts/components/account-quota-panel/ResetCredits.vue` | 接入：`frontend/src/views/accounts/components/AccountQuotaPanel/ResetCredits.vue` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 151 | `frontend/src/views/accounts/components/account-quota-panel/index.vue` | 接入：`frontend/src/views/accounts/components/AccountQuotaPanel/index.vue` | 额度点数：沿本地 owner 同步字段、行为或相关回归 |
| 152 | `frontend/src/views/accounts/index.vue` | 接入：`frontend/src/views/accounts/index.vue` | 界面/夹具：沿本地 owner 同步字段、行为或相关回归 |
| 153 | `frontend/src/views/keys/components/ApiKeyFormModal.vue` | 接入：`frontend/src/views/api-keys/components/ApiKeyCreateModal.vue` | 安装种子：沿本地 owner 同步字段、行为或相关回归 |
| 154 | `frontend/src/views/keys/composables/useApiKeyEditor.ts` | 接入：`frontend/src/views/api-keys/composables/useApiKeyMutations.ts` | 安装种子：沿本地 owner 同步字段、行为或相关回归 |
| 155 | `frontend/src/views/plugins/components/PluginVersionCard.vue` | 保留/排除 | 插件页面已排除 |
| 156 | `frontend/src/views/proxies/index.vue` | 接入：`frontend/src/views/proxies/index.vue` | 界面/夹具：沿本地 owner 同步字段、行为或相关回归 |
| 157 | `frontend/src/views/settings/components/ConcurrencyPolicyCard.vue` | 接入：`frontend/src/views/settings/components/RuntimeSettingsCard.vue` | 沿本地设置页，独立秒数、开关和校验一起接入 |
| 158 | `frontend/src/views/settings/components/TokenRefreshCard.vue` | 接入：`frontend/src/views/settings/components/RuntimeSettingsCard.vue` | 现有 TokenRefresh 区域增加明确秒单位与无障碍文案 |
| 159 | `frontend/src/views/settings/components/backup/BackupStorageCard.vue` | 接入：`frontend/src/views/settings/components/backup/BackupStorageCard.vue` | 界面/夹具：沿本地 owner 同步字段、行为或相关回归 |
| 160 | `frontend/src/views/settings/components/error-protection/AccountErrorProtectionCard.vue` | 接入：`frontend/src/views/settings/components/error-protection/AccountErrorProtectionCard.vue` | 界面/夹具：沿本地 owner 同步字段、行为或相关回归 |
| 161 | `frontend/src/views/settings/composables/useSettingsForm.ts` | 接入：`frontend/src/views/settings/composables/useSettingsForm.ts` | 排队/设置：沿本地 owner 同步字段、行为或相关回归 |
| 162 | `frontend/src/views/settings/composables/useSettingsFormRules.ts` | 接入：`frontend/src/views/settings/composables/useSettingsFormRules.ts` | 排队/设置：沿本地 owner 同步字段、行为或相关回归 |
| 163 | `frontend/src/views/settings/index.vue` | 接入：`frontend/src/views/settings/components/RuntimeSettingsCard.vue` | 本地设置表单集中在 RuntimeSettingsCard，无第二个设置入口 |
| 164 | `release/notes.md` | 保留/排除 | 不以官方发行说明覆盖 NewCPR 发行记录；本轮记录写入本文 |
| 165 | `release/version.yaml` | 保留/排除 | 不改 NewCPR 构建版本，不触发正式发版 |

## 验证记录

验证基于本次工作树最终实现；Rust 使用仓库固定的1.99.1。首轮覆盖受影响包后，只重跑发生修正的模块；下表不把首轮失败写成全量成功。

| 验证 | 实际结果 |
| --- | --- |
| Rust 编译/静态检查 | `cargo check --workspace --locked`、workspace 全目标/全特性 Clippy、`cargo fmt --check`通过；本机 macOS。 |
| Core、xAI | Core 7库测试+541集成测试、xAI396集成测试通过。 |
| Admin/API | 首轮分别402/623通过，旧设置夹具及共享数据构造失败已修正；对应模块40/46例复跑全部通过。 |
| OpenAI | 329库测试通过；首轮1225集成测试通过，7处旧断言/取消夹具失败已修正；点数开关、OAuth、种子及取消相关模块复跑通过。 |
| Host/App | Host23库测试通过、1例既有忽略；首轮126集成测试通过，旧发现/通道夹具修正后63例通过；App14项架构检查通过。 |
| PostgreSQL/Redis | 隔离Linux环境：账号存储109例、设置27例及0086迁移通过；旧设置fixture修正后连同snapshot、Redis排队/亲和62例通过。关闭点数开关后的迟到写不可重新放行。 |
| CI迁移补查 | PostgreSQL18重现4处旧迁移夹具失败；修正后覆盖历史名称、共享排队与Excel/State升级的11例全部通过。其他511例在补查中通过；本地备份恢复例因缺pg_dump未通过，该例在同一原提交的GitHub CI已通过。 |
| Live Linux | 7例全部通过，含官方TLS回环冻结回归；合成凭据、临时证书及回环代理，未访问真实上游。 |
| 前端 | typecheck、build、ESLint通过。首轮1112/1149通过；受影响模块98例复跑95通过，余下3项修复后所在9例全部通过。旧超时测试名随canonical字段更新，组名用例恢复实际组件加载。 |
| 页面交互 | 独立Chrome、1440/390宽度通过：点数保存失败回退与成功保存、种子生成保存、beta切换/刷新、历史失败后再次更新；无横向溢出，主题截图已检查。 |
| 发布/CI工具 | Ruby发布/平台回归通过；新增通道发布顺序及恢复回归9例72断言通过，CI工具18例通过，actionlint通过。未触发正式发布。 |
| 迁移冻结/隐私 | 已发布迁移及原清单条目不变，仅追加0086。暂存内容隐私检查通过；仅对既有回环代理合成凭据逐行重新审查记录，未扩大扫描豁免范围。 |

原始日志、临时浏览器夹具及隔离环境脚本均不进入仓库。GitHub CI与安全扫描针对待合并提交执行，
最终PR保存结果；本地通过不替代该门禁。源码回环与合成上游不代表真实付费请求、真实上游 Live 接受
或生产部署已验证。0086含schema变更，后续部署须使用既有迁移升级流程，不能走无迁移快速路径。
