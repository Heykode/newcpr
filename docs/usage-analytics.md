# 用量统计读侧合同

## Key 与模型

管理员接口 `GET /api/admin/usage/insights/diagnostics` 新增
`dimension=keyModel`。沿用既有管理员认证、时间范围和筛选规则，不增加
客户端公开查询入口，不返回明文 Key。

- 该维度默认 `currentPage=1&pageSize=20`，页码必须大于零，每页 1 至 100 项。
  响应包含 `currentPage`、`pageSize`、`hasMore`；超出末页返回空数组。
  其他维度仍为最多 100 项的热点诊断，不接受分页参数。
- 分组身份是 JSON 编码的 `[client_api_key_ref, model]`，不能按展示名称合并。
  模型优先使用 `upstream_model_id`，缺失才回退 `requested_model_id`；
  两者均缺失时不计入此维度。当前 Key 名称仅用于展示，删除后回退保存的 ID。
- 按请求数降序、分组键升序分页；多币种属于同一条分组，不能拆成多页。
  每次多读一条分组判断下一页，不在第 100 组处截断。
  相同数据上的排序稳定；各页独立读取，不承诺并发新请求期间的冻结快照。
- 已知金额继续在 PostgreSQL `numeric` 与 `DecimalAmount` 中累加和投影，
  未知金额保持 `null`，真实零保持零。`costIncomplete` 标识缺失或部分费用，
  不用零替代缺失。不同币种不相加或换汇。
- 请求终态、健康、重试计数和完成用量谓词保持原合同。Token 与费用只计算
  已完成的推理事实；历史预热不进入这些小计，仍可出现在诊断请求计数中。
  风险及请求占比仍由当前返回集合计算，Key/模型页不展示这些页内相对指标。
- 前端分页只刷新诊断。切换维度、Provider 或时间范围回到第一页；
  取消或过期响应不能覆盖新查询。Key 名和模型名可换行，不能被截断为同名。
- Provider 或时间范围改变时清除旧条件的诊断结果及分页；新第一页失败不能
  继续点击旧第三页的上一页或下一页。同一条件下翻页失败保留原页并允许重试。
  维度切换可显式重新读取当前筛选条件；日志表的刷新仍只刷新日志表。

## 分组归属

分组列表的今日及保留期消费、监控的最近一分钟消费，均根据实际服务账号
`provider_account_ref` 在读取时的当前成员关系归属，不再使用请求的
`routing_group_refs` 作为统计依据。

- 全池 Key 的请求也计入实际账号所属组；路由范围中的非服务组不计入。
- 一个账号属于多组时，每组各计一次，不因其他成员、Key 绑定或路由组关联倍增。
- 成员移入、移出和删除会改变仍保留历史请求的分组展示，不重写请求路由快照、
  历史账单、累计账本或预算结算。
- 列表仍使用原保留期和上海自然日的请求开始时间边界；监控仍使用原完成时间
  `(now - 60s, now]`、原完成谓词和原未知费用处理，不统一或移动这两个时钟。
- ETA 保留现有按账号去重的消费输入；不得用各组消费之和替代。
  无新增推理路径读取、上游调用、额度预测缓存或采样周期。

## 回归入口

- Store：`key_model_diagnostics_page_stable_pairs_with_complete_currency_groups`、
  `list_and_monitor_attribute_all_scopes_to_current_serving_memberships_once`，
  以及既有状态缺失的 HTTP/WS、预热排除、生命周期与金额回归。
- Admin：`key_model_pages_preserve_store_order_and_partial_exact_costs`。
- API：`key_model_diagnostics_*` 与 `diagnostics_pagination_*`。
- 前端：`frontend/tests/usage-key-model.test.mjs`。

Store 回归需要真实隔离 PostgreSQL；缺少环境而提前返回不构成已验证。
本合同没有数据库结构或价格变更。
