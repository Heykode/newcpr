# BPS Excel 退役边界

当前版本不提供 BPS Excel 执行链路。移除范围与兼容约束见 `docs/excel-removal.md`。
不要重新装配 Excel Provider、图片中转、回放、暂停恢复或质检开关。

- 原生 Codex、AGY/xAI、并发、WS 建连预算、身份和代理逻辑保持各自合同。
- 旧路由与会话标识只用于隔离、拒绝错误续接和历史读取，不可回退到另一条链路。
- 旧模板只清理明确列出的退役字段，保留普通配置、版本、引用与 CAS。
- 保留历史迁移与日志；不清空共享 Redis，不整份删除混合模板。
- 共用 `excelFailureThreshold`、`excel_streak`、计费 `multiplier_bps` 不是专属 Excel 功能。
- 修改和验证细节见同一说明；所有测试在用户指定验证主机进行。
