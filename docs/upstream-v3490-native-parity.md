# 官方 v3.49.0 原生功能接入记录

## 固定基线与边界

- NewCPR 基础：`552ce23f6d8967ae1c373ac1cee23ef7f06e329e`（PR #271）。
- 提交前整合主线：`403e359d7042332c10df0dff7dc6ce363cf6606d`（PR #272，Codex / Excel 双凭据及独立登录恢复）。
- 上次官方：`975b73b33b6d247fbe08a9a23e88066467e71d7a`。
- 本次官方：`0f5bf03e13a43b34c43b9e3a6dee935ad43f6d2b`。
- 目标仓库为 `Heykode/newcpr`。提交、推送和主线合并以关联 PR 状态为准；本轮不包含部署，未读取生产版本。
- 按 `upstream-integration-checklist.md` 执行；本文件仅记录固定版本证据，不另立合并规则。

## 官方变更与真实落点

| 官方提交 | 本地接入 | 保留差异与原因 |
| --- | --- | --- |
| `41f97ac` 客户端版本检查与缓存续期 | `transport/profile/{mod,cli_release,platform_release/mod}.rs` 采用官方小时检查、每日续期、30 天缓存及原核验时间；`provider/workers.rs` 注册 CLI、平台 Desktop 和既有 macOS Desktop 任务 | 本地旧自定义 UA 自动同步接入同一个 npm 服务；删除独立 GitHub 六小时抓取服务。只读旧缓存时保留原版本/sequence 校验，固定 UA 不改变。CLI/平台任务逐进程更新内存画像，保留无 leader lease 的本地接入，不能据此宣称支持整套应用多副本部署 |
| `a0e7f38` Claude 明确会话亲和 | 官方 `gateway-protocol/anthropic/session.rs` 原样接入；API 透传会话头；Provider 在 Messages→Responses 转换前解析身份，选号和安全观测共同使用 `claude-session` 命名空间 | 本地只有 OAuth→Responses 链，沿用真实租约与绑定存储；Messages 不再进入转换后正文缓存亲和，不创建 Codex turn/续接归属。官方插件 attempt 改写复验及上游 API Key Messages 路径按既有边界排除 |
| `d13a4a2` 独立额度重置系数 | 官方 `account/scheduling.rs`、严格重置排序及正反例；Admin/API/Store→SnapshotSettingsFacts→冻结 AccountSelectionPolicy→前端；新增迁移 0093 | 保留本地 `preferHigherWeight`、有效账号容量计算和 inline 设置布局。智能调度与额度调度权重分别持久化。全零保留旧排序；不添加新的亲和、队列或重试限制 |
| `779ccf4` PWA 标题栏、主题色 | manifest、App、layout、theme store 与全局样式接入 | 适配本地 cp 主题变量、BaseToast、页面槽和移动侧栏，保留账号分组及原页面布局 |
| `0f5bf03` PWA 高度修复 | 外窗/像素比变化清除旧高度，visualViewport/geometrychange 稳定后更新正文高度，完整接入官方后续修复 | 浏览器检查见下；未把模拟几何检查写成真实系统窗口拖动测试 |
| `4ee9abf`、`28c8c53` 版本与发行通道 | 不覆盖 NewCPR 版本和发行索引 | 发布标签和通道由本仓库发布流程单独处理 |

本轮排除继续包括插件、上游 API Key 账号及其专属 CPR 转发、PRISM；此前暂缓的 analytics worker 不在范围内。

## 旧适配、测试与规范调整

- 退役 `transport/profile/cli_version.rs` 及其独立抓取测试，用官方版本服务测试覆盖旧 UA 的 opt-in、两处版本替换、固定值保护和旧缓存校验。启动检查不再受旧六小时节流阻挡。
- Messages 原先经转换后可能按正文生成 OpenAI 缓存亲和；现按官方显式 Claude 身份处理，无身份不绑定。安全观测也使用同一解析，避免日志与实际选号不一致。
- 新增设置字段完整穿过生产读写、审计字段、快照、API 与页面，不为兼容旧测试给更新请求增加默认值。旧页面或更新脚本须重载并提交 `quotaResetWindowWeights`。
- 迁移 0088/0091 的旧整行保护测试固定到各自迁移终点；读取新版 Repository 前再升级当前 schema。0093 单独验证新增默认值、旧字段保留和实际持久化，未改任何已发布 SQL。
- 更新客户端连续性、协议、额度调度和前端规范以及 API/架构/部署说明，删除相互冲突的六小时抓取及“额度策略永远不变”条款。
- 官方测试使用本地真实容量租约接口与已注册账号范围；没有为测试恢复旧准入逻辑。临时移植、浏览器、负向对照和数据库脚本及原始日志均在仓库外。
- 整合 PR #272 时，保留 Excel 独立凭据刷新、初始化、冻结路由、凭据修订与状态反馈隔离；共用 worker 测试同时覆盖 Excel 的 30 秒刷新和客户端版本的每小时检查。主线已使用迁移 0092，本次未发布的额度权重迁移顺延为 0093，原 SQL 内容不变。保留新版设置页退出 Excel 静态模型列表的逻辑。

## 验证证据

验证环境：本地 Rust 1.99.1、已安装前端依赖、隔离 PostgreSQL/Redis，数据库测试设置 `CI=true`。
编译缓存磁盘不足已清理可再生增量目录，后续使用 `CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=1`；未修改仓库构建配置。

以下结果对应整合 PR #272 后的源码；未受主线整合影响的 Protocol 测试复用首轮结果。静态检查和测试编译覆盖 workspace，运行测试按本次受影响范围选择。

| 范围 | 验证内容与结果 |
| --- | --- |
| Core | 127 项账号/快照测试通过：官方重置排序正反例、亲和优先、缺失窗口、策略独立、冻结计划和双凭据隔离 |
| Protocol | 43 项 Anthropic 测试通过：显式头、新旧 metadata、空/非法标识、普通用户与正文反例，以及既有转换 |
| API/Admin | 51 项 API 设置/Messages 与 68 项 Admin 设置、账号和双凭据重登测试通过：头透传、完整读写、只读 defaults 拒绝、非法数值/null/缺字段不写入、分通道重登结果保留 |
| Provider | 206 项通过：Claude 三种会话身份与 Client Key 隔离、无正文亲和/续接别名、观测与选号同源、严格等待/允许重试后重绑、优先模式临时溢出，以及共享版本服务、旧 UA opt-in、固定值保护、旧缓存校验、Excel 冻结路由和独立刷新 |
| Store | 58 项真实隔离 PostgreSQL/Redis 测试通过：运行设置、快照、artifact 缓存、Excel 独立凭据和生产迁移；数据库测试未跳过 |
| 构建与架构 | workspace 全部测试目标编译、全 workspace/all-targets Clippy（拒绝 warnings）及格式检查通过；14 项 App 架构和 24 项 Host worker/lifecycle 通过 |
| 前端 | 136 项设置/UA、Excel、重登和入库进度回归通过；vue-tsc、Vite 生产构建、受影响文件 ESLint 通过 |
| 浏览器 | Chrome 实际页面保存/重载、桌面及 390px 布局、暗亮主题与 theme-color 同步通过；注入 PWA 显示模式/标题栏矩形，验证 30/44px 留白、短窗/移动窗、列表底部、侧栏和通知不遮挡，无页面错误。manifest 可解析；隔离浏览器安装与启动成功，但 headless 未激活原生 overlay，未验证系统窗口拖动及真实缩放 |
| 迁移与隐私 | 主线旧 SQL 和冻结清单既有行未改变，新增 0093 哈希正确且版本号唯一；隔离数据库从 0092 导出/恢复后升级 0093，与直接升级结果一致，保留自定义设置并新增全零权重；候选源码 tree 及整合后的暂存内容隐私检查通过 |

整合后的双凭据浏览器用例也通过，覆盖桌面/移动布局、账号开关与连接测试、分通道重登、Excel 材料替换、导入和导出。本地 Vite 缓存过期导致的一次页面未加载已通过独立临时缓存复验排除；没有修改产品代码来绕过检查。

负向对照曾临时恢复旧生产选号和排序实现：Claude 不同会话被正文亲和混同的回归、官方额度排序正反例均按预期失败。恢复本轮实现后，Provider 198 项和 Core 123 项最终测试全部通过；对照脚本和原始日志未进入仓库。

未请求真实上游推理、未读取或更改生产服务。本轮证据不能保证所有运行场景零缺陷；提交、推送、主线合并和部署状态须单独核对。
