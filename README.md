# NewCPR

支持 Responses、Chat Completions 和 Anthropic Messages 请求，使用管理端创建的客户端密钥。
Messages 与输入 Token 计数、额度上限、Cron 预热及账号目录的使用边界见 [接口文档](docs/api.md)。
本次官方源码接入与必要差异见 [v3.45.1 合并记录](docs/upstream-v3451-native-parity.md)。

## 开发与官方更新合并

提交前先阅读 [AGENTS.md](AGENTS.md)；官方更新按
[合并核对清单](docs/upstream-integration-checklist.md) 和对应 `.trellis/spec/` 执行。
规则随仓库版本维护，新电脑或新会话先拉取目标分支；历史差异账不能代替当前规则。
