# NewCPR

支持 Codex、xAI 和 Antigravity OAuth 账号，使用管理端创建的客户端密钥。
Codex/xAI 提供原生 Responses，Antigravity 提供原生 GenAI；Chat Completions 按所选平台转换。
模型、Token 计数、额度及现有兼容路由的使用边界见 [接口文档](docs/api.md)。
本次官方源码接入与必要差异见 [v3.57.2 合并记录](docs/upstream-v3572-native-parity.md)。

## 开发与官方更新合并

提交前先阅读 [AGENTS.md](AGENTS.md)；官方更新按
[合并核对清单](docs/upstream-integration-checklist.md) 和对应 `.trellis/spec/` 执行。
规则随仓库版本维护，新电脑或新会话先拉取目标分支；历史差异账不能代替当前规则。
