# 官方 v3.39.1 原生对齐

## 固定来源

- 本地基线：`f19a189b45f6b6f30668829069f073268e0e9e9d`，与主线 `3787fd98` 同树。
- 官方上一基线：`96e8443ede051abdd8f0bac69462a7cfe2945633`。
- 官方本轮：`12c32fcdecb29da4e44a21bb33fbbf2aaf6a166d`，六项修复及版本说明。

## 差异与接入

| 官方来源 | 原行为 | 本轮行为与必要接入 |
| --- | --- | --- |
| `790f3147` | Unknown 归属的完整请求清除所有 turn-state | 移植摘要证明缓存，覆盖 HTTP 成功/失败头和 WS metadata；仅匹配 Key、账号、会话、线程、轮次的 token 可回传，不能授权其他状态 |
| `2016bda0` | 追踪字段在多处被删除，fedramp 被视为受管头 | 删除 API、Provider、最终投影、无损 JSON、Live 的旧过滤；保留重复业务/追踪头及 metadata |
| `2016bda0` + `e7c24781` | 安装号只写正文，后处理删安装头 | 共用账号持久安装号；最终投影也只取受信 context。地域和账号路由仍由部署配置控制 |
| #584 / `26de6712` | 事实版本竞争直接失败；四轮预算只处理目录变化 | 四轮内重读完整事实，返回最后的不稳定原因，存储失败立即保留；原生无插件，不引入扩展准备系统 |
| `3b4b547e` | 删除冲突已显式回滚，但回滚失败掩盖冲突 | 与已有更新路径一致，保留 Conflict 并附清理失败来源；覆盖暂停连接归还时连续编辑/删除 |
| #585 / `a92b95c4` | 显示双栈总体耗时 | 分别计时，显示首选成功 IPv4、否则 IPv6 的耗时；仍等两族完成，不改变模型请求出口选择 |

状态证明沿用本地 `openai_session_binding_ttl`，对应官方会话亲和 TTL，默认均为
24 小时；容量保持官方 4096 条，仅存摘要及过期时间。原生密文修复会克隆请求，
因此证明 scope 也可克隆，归属不变。OAuth 刷新不改变账号归属；项目无上游 API Key
账号，故不引入官方该认证类型的 revision 分支。

负例发现原生完整请求的 `lease.affinity_hit()` 被误当作所有旧状态的归属证明，
使不同轮次 token、未知 response ID 和 metadata 状态直接保留。本轮按官方最终
判断去掉这一原生旁路，并纳入 pinned continuation 的显式 owner；只选择账号的
粘性调度本身不变。Excel 属于另一协议，不改变它既有的状态合同。

Guardian 的父响应可信性仍单独校验。普通粘性命中不构成父响应证明，turn-state
证明也不恢复其他字段。安装号由既有档案提供，不另建身份生成器。追踪头保留是
官方明确取代旧规则，不能将历史清理测试作为新行为标准。

快照最终 revision 检查在官方仅用于插件扩展准备，本项目不新增这一插件分支；
原生发布器仍执行其原有发布版本控制。目录变化的预算与事实重读共享，不额外重放
用户请求。不修改数据库迁移、依赖、前端设置、生产参数或发布流程。

## 调用链与验证映射

| 边界 | 接入及回归 |
| --- | --- |
| 普通 Responses 与全量重放 | `CodexProvider::execute` 建立证明 scope，`scope_request_to_account` 只放行匹配 token；`provider::contract::turn_state` 覆盖正常、换 Key/账号/会话/轮次、过期和未知 response ID |
| 失败响应与 WS | `cold_response_stream_once` 记录 HTTP 成功/错误头；三处 `merge_response_metadata_updates` 汇合 WS metadata；验证成功完成与先返回 metadata 再断开两种情况 |
| 请求头与正文 | API decoder、`compat::headers`、`AccountCorrelation`、raw JSON、QX 最终投影和 Live 同步；`identity_isolation`、`raw_identity`、`qx_application` 验证追踪字段、重复成员和账号身份 |
| 安装号与部署路由 | `model_request_headers` 覆盖 Responses、模型目录、预热与 Live；`native_catalog_local`、`quota::warmup`、`qx_profile`、`profile_contract` 验证真实回环 HTTP/WS 请求 |
| 配置发布 | `RuntimeSnapshotCompiler::compile_inner` 重读完整事实；`compiler_reloads_complete_facts_after_revision_changes_during_read` 和 `committed_refresh_reloads_raced_facts_without_losing_revoked_scope` 验证重读及权限撤销 |
| 代理数据库冲突 | `PgProxyRepository::delete` 显式 rollback 并保留原错误；`stale_proxy_mutations_release_locks_before_next_update` 暂停连接归还，验证过期检测、编辑、删除不会占住后续编辑的锁 |
| 双栈探测 | `HttpProxyProbe::timed_exit_ip_at` 分别计时；`dual_stack_latency_measures_the_selected_exit_without_the_other_family_wait` 覆盖 IPv4/IPv6 各自成功且另一族慢失败 |

本地 Excel 路由保留自己的状态清理与粘性合同，不接入官方插件扩展、上游 API Key
认证 revision 或 PRISM。安装号缺失/显式空值、部署路由缺失/空值/自定义画像的
行为跟随官方，不添加额外默认值或策略开关。

## 验证

执行性验证仅在授权的隔离 Linux 环境，使用合成账号、回环上游和独立数据库。
旧版负例与新版验证切换时清理受影响包缓存，防止复用旧库导致假结果。

2026-11-03 已完成：

| 验证 | 结果 |
| --- | --- |
| 旧代码负例 | 配置重读两项、双栈计时一项、状态归属四项均出现预期断言失败；编译失败不计入复现 |
| `cargo check --workspace --all-targets --locked` | 通过 |
| `cargo fmt --check` | 通过，格式化结果已同步回工作树 |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | 通过 |
| gateway-api | 620 项通过 |
| gateway-core | 535 项通过，含上述旧版失败用例 |
| gateway-host | 136 项通过；1 项既有 Mihomo 内核下载/网络命名空间测试保持 ignored |
| gateway-store | 643 项通过，实际连接独立 PostgreSQL 18 和 Redis 8；代理冲突释放锁用例通过 |
| provider-openai | 1,520 项通过，含全部六项 turn-state 集成测试、HTTP/WS/Live、目录、预热、换号与续接 |
| 源码一致性 | 后端文件逐字节校验无差异；空目录差异不涉及源码 |

合计 3,454 项通过、0 项失败、1 项既有 ignored。数据库备份适配器在子进程重跑的
一项测试包含在所属测试内，不重复计数。测试容器已清理，原始日志保留在仓库外。
代理回滚失败的错误组合沿用已有更新路径；未故障注入数据库断连来单独触发
rollback 失败。未进行真实上游账号请求、生产部署或风控/TPS 效果测量。
