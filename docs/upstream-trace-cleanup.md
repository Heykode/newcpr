# 出站追踪字段对齐

> 历史记录：下述清理规则已被官方 v3.39.1 的客户端头保留规则取代。
> 当前行为与验证见 `upstream-v3391-native-parity.md`，不能据此重新删除追踪字段。

本轮 NewCPR 基线 `8aee0f787309d85fcbb777962d4e6fd19630f5f9`，固定官方提交
`c57af9b0dd95428201d59adf17367b8f651f73f0`，位于上次吸收的 `641f43b3` 之后。

## 来源与处理

| 官方变更 | 本地处理 | 行为边界 |
| --- | --- | --- |
| correlation 字段表 | 原样加入四个字段 | 删除 sentry-trace、baggage 及对应 WS metadata 副本 |
| correlation HTTP/WS 测试 | 接入原生 identity_isolation 和 qx_application 测试 | 实际发送、大小写与重复头、对象/字符串 metadata、turn 保留 |
| privacy WS 测试 | 吸收适用的复用和隔离断言 | 追踪字段变化保持缓存键和连接复用；账号/Key/UA 身份变化保持隔离 |
| API 文档 | 更新固定出站清理说明 | 不引入用户明确排除的可配置隐私规则及其专用测试 |

运行时代码只修改共享字段表，不修改身份派生、调度、UA、费用或精确续接规则。
Responses 的最终 headers 在 `apply_response_headers` 清理后才进入 WS connection profile；
正文沿既有 correlation 投影清理。独立 Search/images/compact 继续使用原始 JSON span 编辑，
Live 继续使用同一请求头清理函数。新增回归覆盖这些共享调用方。

这不是整套官方仓库替换：原生账号/Key 安装身份映射、无损 JSON 和既有连接池架构保留。
官方隐私策略修改 Authorization 的专用场景不引入；采用已有账号/Key/UA 切换验证
原生连接隔离，保留原有精确续接测试。没有真实上游风控效果的测量结论。

## 验证

仅在 OVH 隔离环境使用合成请求和 loopback 上游，不运行本机测试，不操作生产。

| 检查 | 结果 |
| --- | --- |
| 修复前回归 | 两项独立失败：请求投影保留新追踪字段；实际 HTTP 发出 sentry-trace |
| 修复后协议回归 | 52 通过、0 失败、0 跳过；覆盖身份投影、HTTP/WS、独立端点、连接复用、UA/精确续接、HTTP 回退及透支开关既有行为 |
| Live 共享头处理 | 1 通过、0 失败、0 跳过；重复追踪头清理及业务会话引用保留 |
| 格式检查 | `cargo fmt --all --check` 通过 |
| 静态检查 | `cargo clippy --locked -p provider-openai --all-targets -- -D warnings` 通过；已移除新增测试中的多余克隆 |
| 源码一致性 | 五个 Rust 变更文件的本地与 OVH SHA-256 完全一致 |

协议回归命令：

```sh
cargo test --locked -p provider-openai --test main -- \
  transport::qx_application:: provider::contract::identity_isolation:: \
  provider::contract::raw_identity:: transport::qx_profile:: \
  provider::contract::request_alignment:: provider::contract::ws_size_recovery:: \
  exact_websocket_busy --test-threads=2
cargo test --locked -p provider-openai --lib \
  live_headers_protect_current_identity_and_frozen_profile
```

没有运行真实账号请求或生产抓包；不将 loopback 验证描述为真实上游验收。
