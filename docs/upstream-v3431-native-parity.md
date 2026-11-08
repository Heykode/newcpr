# 官方 v3.43.1 合入与差异账

## 固定范围

- 本地起点：`ca49739113ab55acd1e557861f46936cef196a11`（NewCPR 3.42.2）。
- 上次官方：`f772bda57b62842e7bfca0d385d9fd6099a168b3`（3.42.1）。
- 本次官方：`e7ef31d882deb033538a23643779a9f41328c46a`（3.43.1），11 个主仓提交、70 条最终变更路径。
- UI 固定目标：`61ceb0c0130463510fb922a554a398dee6b71a86`（0.9.0），相对前次 0.8.1 有 2 个提交、32 条变更路径。
- 本账记录源码合入及本地隔离验证，不代表已发布、已推送或已部署；不复制官方发布通道/资产声明。
- 共用规则仍为 [官方更新合并核对清单](upstream-integration-checklist.md)。本账只保存本次来源、接入、保留差异与证据。

## 实际行为与必要接入

| 来源 | 官方变化 | NewCPR 实际落点与差异 |
| --- | --- | --- |
| `ff1ea921` | 监督执行租约，停机主动释放，2 秒有界 drain | 官方 `SchedulingLeaseCleanup` 原实现移入 `redis/scheduling_cleanup.rs`，仅调整 shutdown 可见性。接到生产使用的 `RedisProviderLeaseCoordinator` → `RedisCapacityWait` 直接准入/晋升、`Ownership::supervise` 和原 `CapacityWaitCleanupWriter`；bundle/workers 已注册该 writer。保留本地 Redis FIFO、Shared/Reserved、续期、取消 tombstone 与 TTL 兜底，不再保留无限 Store drain 旧合同 |
| `1a2a4a56` | 人工 OAuth 更新 user/workspace、清旧身份状态，保留本地名称；隔离缓存和续接 | 直接采用官方 `prepare_oauth_rotation`、identity hash、Session/Guardian/avatar 分支；实际 callback 和 PG authorization adapter 接入。账号 client 和本地 session-proxy client 均追加身份 key；设备归档与 State 事务适配见下文 |
| `e770c94a` | 图片具名额度不覆盖 Codex 周窗口 | 使用官方完整 active-limit 分支：目标非 codex 且 map 不含该 key 才迁移；删除本地 premium/name-only 特判；协议正反例及 Provider 完整窗口/访问状态断言同批更新 |
| `d9f9eca7` / UI 0.9.0 | 四场景预设、默认/柔光材质、共享侧栏及组件修复 | 移植官方实现及所需的现行主题颜色/对比度依赖，接入实际 Store/编辑器/Base 组件。保留 `--cp-*` 覆盖键、原持久化键与 11 个本地菜单；账号分组保留原多文字标签组件 |
| `290aecb7` | 主滚动层不透明底色 | 本地 `BaseScrollbar` 无官方 wrap class，背景加到实际首个滚动 div；不是只给不滚动的外壳加色 |
| `8b9aacd8` | 区分备版和通道更新提交标题 | `release/publish`、`release/published-channels.rb` 和必要测试对齐；部署文档补旧历史标题兼容核对；不改变精确提交 CI 和制品复用流程 |
| `ce620f1b` | Intel macOS 平台 | 按批准范围补 `darwin/amd64`、`x86_64-apple-darwin`、`macos-15-intel` 恢复说明；默认仍仅 Linux AMD64。未构建该平台，不宣称已发布 Intel 资产 |
| 4 个版本/通道提交 | 官方 3.43.0、3.43.1 发布元数据 | 记录来源，不写官方 published SHA、资产或版本号到 NewCPR 的当前版本 |

## 适配中额外核实的问题

1. 本地设备表原先禁止账号换 workspace；人工 OAuth 如仍走该限制，会抵消官方更新。
   现在人工重授权允许新 user/workspace，保留所选槽位 installation ID，实际活账号重复仍回滚。
   另用真实 PG 复现了“目标账号已删除，仅设备归档存在仍拒绝”的问题；修复前失败、修复后通过。
   只在明确人工重授权事务中替换无活账号引用的目标归档，普通导入继续复用历史设备。
2. 本地 SQL 曾保留人工更新的旧 quota snapshot，及旧测试把同身份也强制写 `replacement_identity`。
   现按官方清快照，实际换身份才清 quota access/evidence/reset；同身份 RT 续期保持原合同。
3. `OpenAiAccountUseCase::complete_authorization` 已有提交后的 `observe_initial_quotas`，包含重授权；
   之前文档只写首次创建，现修正文档，没有再加第二个重复额度任务。
4. 新官方 UI 使用无前缀 token 名，而本地存储/CSS 使用 `--cp-*`；补齐预览、覆盖白名单与
   派生变量列表的实际消费转换，避免“主题有实现、列表却为空”。
5. 本地有未保存离页确认。侧栏的点击选中先恢复真实 route，取消不会错误高亮；
   手机先收起侧栏遮罩和焦点锁，避免确认框被挡。浏览器真实点击复现后修复，保留既有布局回归。

## 保留边界

- 本批没有更改在线 strict/prefer/loose、根/子线程绑定、FIFO、等待人数、等待时长或换号预算。
  停机释放不等于解决在线冷却账号的严格跟随等待，不据此承诺既有 429/503 根因消失。
- 本地文件替换/自动重登继续走其已批准的 email/workspace 合同；人工 OAuth 不再使用该预检查。
- 账号安装 ID、出口政策、Excel 和托管 State 是 NewCPR 现有能力。真正执行路径都消费新身份，
  旧 State 的隔离仍由现有 binding revision 完成，不增加新迁移。
- 已排除插件、peer_link 和上游 API-key 产品；未存在的 pricing/key-usage/DatePicker 页面不凭本次小样式补丁新造产品。
- 没有引入数据库 migration，没有改生产设置、队列限额、版本号或部署流程。

## 逐路径核对：官方主仓

“逻辑移植”表示采用本次 patch 的真实实现；不等于整个本地文件与官方字节相同。
“必要接入”包含现有后端/组件 API/产品差异，实际影响在上表和适配记录中说明。

| 官方路径 | 处理与本地落点 |
| --- | --- |
| `.agents/skills/cpr-release/references/operations.md` | 发行规范迁入现有 `deploy/README.md`；本地没有这套官方发行 skill |
| `.agents/skills/cpr-release/references/preparation.md` | 发行规范迁入现有 `deploy/README.md`；本地没有这套官方发行 skill |
| `.github/scripts/published-channels.rb` | 逻辑移植 → `release/published-channels.rb` |
| `README.md` | 平台扩展说明 → `deploy/README.md`；不复制尚未发布的平台承诺 |
| `backend/crates/gateway-admin/src/use_case/plugins/official.rs` | 插件产品按既有范围排除；公共 UI 改动另行接入 |
| `backend/crates/gateway-admin/tests/use_case/plugins/official.rs` | 插件产品按既有范围排除；公共 UI 改动另行接入 |
| `backend/crates/gateway-protocol/src/openai/events.rs` | 逻辑移植/必要接入 → 同相对路径；真实调用链见上表 |
| `backend/crates/gateway-protocol/tests/openai/events.rs` | 逻辑移植/必要接入 → 同相对路径；真实调用链见上表 |
| `backend/crates/gateway-store/src/bundle.rs` | 既有 `capacity_wait_cleanup_writer` 注册链已覆盖；监督器嵌入该 writer，不重复注册 worker |
| `backend/crates/gateway-store/src/postgres/provider_accounts/authorization.rs` | 必要接入 → `postgres/provider_accounts/admin_adapter.rs` |
| `backend/crates/gateway-store/src/postgres/provider_accounts/repository.rs` | 逻辑移植/必要接入 → 同相对路径；真实调用链见上表 |
| `backend/crates/gateway-store/src/redis/credential_leases/cleanup.rs` | 官方实现 → `gateway-store/src/redis/scheduling_cleanup.rs`，shutdown/常量可见性适配 |
| `backend/crates/gateway-store/src/redis/credential_leases/mod.rs` | 必要接入 → `redis/capacity_wait.rs` 与 `redis/mod.rs`，coordinator 委托链保留 |
| `backend/crates/gateway-store/src/workers.rs` | 既有 `capacity_wait_cleanup_writer` 注册链已覆盖；监督器嵌入该 writer，不重复注册 worker |
| `backend/crates/gateway-store/tests/bundle.rs` | 本地无对应文件；实际 Redis coordinator/writer 关停用例验证接线 |
| `backend/crates/gateway-store/tests/postgres/provider_accounts/authorization.rs` | 官方断言接入实际 PG `tests/postgres/provider_accounts/devices/mod.rs` |
| `backend/crates/gateway-store/tests/postgres/provider_accounts/mod.rs` | 换身份清耗尽等断言接入 `devices/mod.rs`；旧 snapshot 断言更新于 `timestamps.rs` |
| `backend/crates/gateway-store/tests/redis/credential_leases/cleanup.rs` | 必要接入 → `gateway-store/tests/redis/capacity_wait.rs`（持有 guard、竞态、他实例、两池） |
| `backend/crates/gateway-store/tests/redis/credential_leases/mod.rs` | 对应回归合入实际 `tests/redis/capacity_wait.rs`，未复制不存在的测试模块 |
| `backend/crates/providers/openai/src/admin/mod.rs` | 本地 `providers/openai/src/admin.rs` 已由 refresh_oauth_credentials 消费新 preparer；移除过时 OAuth 冲突映射 |
| `backend/crates/providers/openai/src/credential/admin.rs` | 逻辑移植/必要接入 → 同相对路径；真实调用链见上表 |
| `backend/crates/providers/openai/src/credential/mod.rs` | 逻辑移植/必要接入 → 同相对路径；真实调用链见上表 |
| `backend/crates/providers/openai/src/credential/oauth.rs` | 逻辑移植/必要接入 → 同相对路径；真实调用链见上表 |
| `backend/crates/providers/openai/src/credential/profile_statistics.rs` | 逻辑移植/必要接入 → 同相对路径；真实调用链见上表 |
| `backend/crates/providers/openai/src/credential/types.rs` | 逻辑移植/必要接入 → 同相对路径；真实调用链见上表 |
| `backend/crates/providers/openai/src/provider/execution.rs` | 逻辑移植/必要接入 → 同相对路径；真实调用链见上表 |
| `backend/crates/providers/openai/src/provider/guardian.rs` | 逻辑移植/必要接入 → 同相对路径；真实调用链见上表 |
| `backend/crates/providers/openai/src/provider/mod.rs` | 逻辑移植/必要接入 → 同相对路径；真实调用链见上表 |
| `backend/crates/providers/openai/src/transport/client.rs` | 逻辑移植/必要接入 → 同相对路径；真实调用链见上表 |
| `backend/crates/providers/openai/tests/credential/oauth.rs` | 逻辑移植/必要接入 → 同相对路径；真实调用链见上表 |
| `backend/crates/providers/openai/tests/provider/contract/mod.rs` | 逻辑移植/必要接入 → 同相对路径；真实调用链见上表 |
| `backend/crates/providers/openai/tests/transport/websocket_pool.rs` | 逻辑移植/必要接入 → 同相对路径；真实调用链见上表 |
| `deploy/README.md` | 更新对应文档，以实际本地入口/默认平台为准 |
| `docs/api.md` | 更新对应文档，以实际本地入口/默认平台为准 |
| `docs/architecture.md` | 更新对应文档，以实际本地入口/默认平台为准 |
| `docs/theme.md` | 更新对应文档，以实际本地入口/默认平台为准 |
| `frontend/package.json` | 本地 UI 内置于 Base/theme，不引入无消费者的 UI 包或子模块；具体实现见 UI 路径表 |
| `frontend/pnpm-lock.yaml` | 本地 UI 内置于 Base/theme，不引入无消费者的 UI 包或子模块；具体实现见 UI 路径表 |
| `frontend/src/i18n/locales/en/appearance.json` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/i18n/locales/en/navigation.json` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/i18n/locales/en/settings.json` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/i18n/locales/zh-Hans/appearance.json` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/i18n/locales/zh-Hans/navigation.json` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/i18n/locales/zh-Hans/settings.json` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/i18n/locales/zh-Hant/appearance.json` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/i18n/locales/zh-Hant/navigation.json` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/i18n/locales/zh-Hant/settings.json` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/layout/components/AppSidebar.vue` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/layout/components/system-update-modal/index.vue` | 必要接入 → `frontend/src/layout/components/SystemUpdateModal/index.vue`（本地命名） |
| `frontend/src/layout/index.vue` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/stores/modules/theme.ts` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/views/accounts/index.vue` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/views/dashboard/components/WireProfileCard.vue` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/views/key-usage/components/KeyUsageSummary.vue` | 本地不存在对应页面；不为标题/尺寸补丁引入新的业务页面 |
| `frontend/src/views/plugins/components/PluginExtensionsPanel.vue` | 插件产品按既有范围排除；公共 UI 改动另行接入 |
| `frontend/src/views/settings/pricing/index.vue` | 本地不存在对应页面；不为标题/尺寸补丁引入新的业务页面 |
| `frontend/src/views/theme/components/AppearanceSettingsSection.vue` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/views/theme/components/ThemeComponentTokenPanel.vue` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/views/theme/components/ThemeEditorPreview.vue` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/views/theme/components/ThemeGlobalTokenPanel.vue` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/views/theme/components/ThemePreviewCanvas.vue` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/views/theme/components/theme-scenes.svg` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/views/theme/composables/useThemeEditor.ts` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/views/usage/components/UsageCostCard.vue` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `frontend/src/views/usage/components/UsageDetailFieldGrid.vue` | 逻辑移植/必要接入 → 同相对路径；Base API、cp namespace 与本地菜单保留 |
| `modules/ui` | 本地 UI 内置于 Base/theme，不引入无消费者的 UI 包或子模块；具体实现见 UI 路径表 |
| `release/notes.md` | 官方发行事实仅记录；不冒用官方版本/通道/资产 |
| `release/platforms.yaml` | 按批准边界保留 Linux AMD64 默认矩阵；Intel 恢复步骤进入部署文档 |
| `release/publish` | 原样变更备版提交标题；本地已有发行证据流程保留 |
| `release/version.yaml` | 官方发行事实仅记录；不冒用官方版本/通道/资产 |

## 逐路径核对：官方 UI

| 官方 UI 路径 | 处理与本地落点 |
| --- | --- |
| `README.md` | 上游 UI 库演示/安装文档不复制；相关行为与调用写入本地 `docs/theme.md` |
| `package.json` | 库版本/发行资料不复制到本地应用 |
| `playground/.vitepress/config.ts` | 上游 UI 库演示/安装文档不复制；相关行为与调用写入本地 `docs/theme.md` |
| `playground/components/card.md` | 上游 UI 库演示/安装文档不复制；相关行为与调用写入本地 `docs/theme.md` |
| `playground/components/color-picker.md` | 上游 UI 库演示/安装文档不复制；相关行为与调用写入本地 `docs/theme.md` |
| `playground/components/date-picker.md` | 上游 UI 库演示/安装文档不复制；相关行为与调用写入本地 `docs/theme.md` |
| `playground/components/index.md` | 上游 UI 库演示/安装文档不复制；相关行为与调用写入本地 `docs/theme.md` |
| `playground/components/sidebar.md` | 上游 UI 库演示/安装文档不复制；相关行为与调用写入本地 `docs/theme.md` |
| `playground/examples/ColorPickerDemo.vue` | 上游 UI 库演示/安装文档不复制；相关行为与调用写入本地 `docs/theme.md` |
| `playground/examples/SidebarDemo.vue` | 上游 UI 库演示/安装文档不复制；相关行为与调用写入本地 `docs/theme.md` |
| `playground/guide/architecture.md` | 上游 UI 库演示/安装文档不复制；相关行为与调用写入本地 `docs/theme.md` |
| `playground/guide/installation.md` | 上游 UI 库演示/安装文档不复制；相关行为与调用写入本地 `docs/theme.md` |
| `playground/guide/theme.md` | 上游 UI 库演示/安装文档不复制；相关行为与调用写入本地 `docs/theme.md` |
| `release/notes.md` | 库版本/发行资料不复制到本地应用 |
| `src/components/card/Card.vue` | 逻辑移植 → `BaseCard.vue`，消费 card-bg-image |
| `src/components/color-picker/ColorPicker.vue` | 必要接入 → `BaseColorPicker/index.vue`，收窄尺寸与主确认按钮 |
| `src/components/sidebar/Sidebar.vue` | 移植真实实现 → `frontend/src/components/base/BaseSidebar/Sidebar.vue`；组件依赖、Popover slot 和 cp 命名适配 |
| `src/components/sidebar/components/SidebarEntry.vue` | 移植真实实现 → `frontend/src/components/base/BaseSidebar/components/SidebarEntry.vue`；组件依赖、Popover slot 和 cp 命名适配 |
| `src/components/sidebar/components/SidebarNav.vue` | 移植真实实现 → `frontend/src/components/base/BaseSidebar/components/SidebarNav.vue`；组件依赖、Popover slot 和 cp 命名适配 |
| `src/components/sidebar/index.ts` | 移植真实实现 → `frontend/src/components/base/BaseSidebar/index.ts`；组件依赖、Popover slot 和 cp 命名适配 |
| `src/components/sidebar/types.ts` | 移植真实实现 → `frontend/src/components/base/BaseSidebar/types.ts`；组件依赖、Popover slot 和 cp 命名适配 |
| `src/index.ts` | 按本地现有直接 import 暴露 BaseSidebar；不新增整个 UI 库公共 barrel |
| `src/locale/lang/en.ts` | 主题文案进入三语言 appearance；Select 默认占位进入 common/BaseSelect。DatePicker 本地无消费者 |
| `src/locale/lang/zh-Hans.ts` | 主题文案进入三语言 appearance；Select 默认占位进入 common/BaseSelect。DatePicker 本地无消费者 |
| `src/locale/lang/zh-Hant.ts` | 主题文案进入三语言 appearance；Select 默认占位进入 common/BaseSelect。DatePicker 本地无消费者 |
| `src/styles/tailwind.css` | 新增 `shadow-cp-menu-item-selected` → `frontend/src/styles/index.css`，其它新字段由既有 Token compiler 生成 |
| `src/theme/core/constants.ts` | 逻辑移植/必要接入 → `frontend/src/theme/core/constants.ts`；类型名、cp Token 与既有 API 兼容 |
| `src/theme/core/normalize.ts` | 逻辑移植/必要接入 → `frontend/src/theme/core/normalize.ts`；类型名、cp Token 与既有 API 兼容 |
| `src/theme/core/resolve.ts` | 逻辑移植/必要接入 → `frontend/src/theme/core/resolve.ts`；类型名、cp Token 与既有 API 兼容 |
| `src/theme/derive/components.ts` | 逻辑移植/必要接入 → `frontend/src/theme/derive/components.ts`；类型名、cp Token 与既有 API 兼容 |
| `src/theme/index.ts` | 逻辑移植/必要接入 → `frontend/src/theme/index.ts`；类型名、cp Token 与既有 API 兼容 |
| `src/theme/types.ts` | 逻辑移植/必要接入 → `frontend/src/theme/types.ts`；类型名、cp Token 与既有 API 兼容 |

本地主题 compiler 还需官方现行 `core/color.ts`、`derive/colors.ts` 的 fill/contrast 依赖；
已一并移入，`derive/appearance.ts`/`roles.ts` 与本地兼容，不保留调用缺失 helper 的半成品。
`BasePopover` 补官方 enterable/focus 行为和 tooltip/dialog role，本地已有 Select 只补默认翻译。

## 验证证据与限制

原始日志、临时对照源码、一次性浏览器场景均留仓库外；下列结果来自实际完成的检查。

- 真实 Redis：32 项容量等待回归通过；本次持有 guard 用例在旧容量实现下超时失败，当前通过。
- 真实 PostgreSQL：110 项 provider_accounts 回归通过；设备目标仅有归档的反例修复前失败、修复后通过。
- Protocol：37 项 events 回归通过；Provider 图片完整窗口/访问状态回归通过。
- OpenAI Provider：首次完整 main 1231 项通过，4 项旧 OAuth/rotation 预期已按官方更新，相关 57 项复跑通过；
  1 项暂停时钟的 socket 关闭时序测试单独复跑通过，产品代码未为此修改。
- 浏览器：1440/390 宽度，四场景、明暗、两材质、旧配置、保存刷新、11 菜单、守卫取消、
  手机焦点、多组标签几何与提示通过；既有主题预览与导航回归通过。
- Release：`release_checks_test.rb`、`publication_test.rb`（14 项/113 断言）、`publish_test.rb`（6 项/120 断言）通过，未执行真实发版。
- 收尾：Rust 全工作区格式检查及受影响三个包 all-targets/all-features Clippy 通过；
  最终代码的 Redis 两项停机回归、PG 24 项设备回归及 OAuth 两项入口回归通过。
- 前端完整 1156 项测试、ESLint、vue-tsc 和 Vite build 通过；无需在后续提交阶段重复这些有效证据。
- 候选变更通过精确临时 index 的隐私检查与 `git diff --check`；没有提交临时脚本、原始日志或生产资料。

以上是本机隔离 PG/Redis 和模拟上游/浏览器证据；没有重新授权真实生产账号、真实上游压力测试、
Linux 制品 CI 或生产部署。不能据此承诺零缺陷，也不能把本次停机修复说成在线排队策略变化。
