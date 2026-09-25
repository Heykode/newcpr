# Backend Development Guidelines

> Best practices for backend development in this project.

---

## Overview

This directory contains guidelines for backend development. Fill in each file with your project's specific conventions.

---

## Guidelines Index

| Guide | Description | Status |
|-------|-------------|--------|
| [Excel Upstream](./excel-upstream.md) | Account-level route, scoped replay, downstream WS and State retirement | Integration verification in progress |
| [Directory Structure](./directory-structure.md) | Module organization and file layout | To fill |
| [Database Guidelines](./database-guidelines.md) | ORM patterns, queries, migrations | To fill |
| [Admin Security And Budget](./admin-security-and-budget.md) | 管理密码撤销、原子审计与预算重置 | 当前分支局部适配 |
| [Error Handling](./error-handling.md) | Error types, handling strategies | To fill |
| [Quality Guidelines](./quality-guidelines.md) | Code standards, forbidden patterns | To fill |
| [分组监控通知](./group-monitor-alerts.md) | 独立渠道、告警确认、事务去重与隔离投递 | 本地实现 |
| [Logging Guidelines](./logging-guidelines.md) | Structured logging, log levels | To fill |
| [Proxy and Responses Delivery](./proxy-and-delivery-contracts.md) | Managed proxies, partial account updates and buffered delivery | Verified PR 60 integration |
| [Account Cumulative Costs](./account-cumulative-costs.md) | 独立累计、请求去重、日志保留及金额容量 | 已验证消费合同 |
| [Quota Forecast](./quota-forecast.md) | Read-only paired quota estimates, inference filtering and UI lifecycle | v3.5 integration contract |
| [Client Continuity](./client-continuity.md) | Default/custom UA, durable devices and opt-in IPv6 routes | Integration verification |
| [Protocol Compatibility](./protocol-compatibility.md) | Chat delivery, explicit compaction and multipart image boundaries | Local compatibility contracts |
| [Scheduling Affinity](./scheduling-affinity.md) | Supplemental hints, checked migration and retry/owner precedence | Local affinity contracts |
| [账号容量等待](./account-capacity-wait.md) | 单一权威、冻结候选与实时容量、共享截止及取消所有权 | 实现合同；专项验收待完成 |
| [Native Model Catalog](./native-model-catalog.md) | 完整原生对象、冻结账号权限、有界缓存与账号出口 | v3.6.1 本地适配 |
| [账号模型限制](./account-model-access.md) | 人工白黑名单、冻结候选与 State 组合、可选补丁和续期保留 | PR 105 本地适配 |
| [Account Relogin](./account-relogin.md) | Login library, create-only import, cancellation and ambiguous push fencing | Local tests; one live Free login passed |
| [Request Location](./request-location.md) | Default-off global and proxy locations without changing account identity | HTTP/WS payload and persistence contracts |
| [Managed Turn State](./managed-turn-state.md) | Opt-in state storage, nonblocking observation and exact WS ownership | Workspace/database regression verified; live opt-in acceptance pending |
| [Stable Upstream Adaptation](./stable-upstream-adaptation.md) | Reasoning replay, recovery, cycle forecasts, account notes and background updates | Local integration contracts |
| [Quota Observation Integrity](./quota-observation-integrity.md) | Passive authority, capture clocks, error quota facts and named buckets | Local/Linux, isolated stores and bounded live HTTP/WS verified |

---

## How to Fill These Guidelines

For each guideline file:

1. Document your project's **actual conventions** (not ideals)
2. Include **code examples** from your codebase
3. List **forbidden patterns** and why
4. Add **common mistakes** your team has made

The goal is to help AI assistants and new team members understand how YOUR project works.

---

**Language**: All documentation should be written in **English**.
