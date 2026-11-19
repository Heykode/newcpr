# 分页状态与仪表盘比较

## 范围

管理端分页失败回退、弹窗异步请求生命周期及部署时区的昨日比较。
不改变账号调度、身份、出口、业务计费或诊断保留策略。

## 接口

- `usePagedQuery.execute({ page?, pageSize?, silent?, background? })` 返回是否接受本次结果。
- `reloadFromStart()` 清空旧筛选的数据和总数，以已确认的每页数量查询第一页。
- `ObservabilityStore::dashboard_card_metrics(TimeRange)` 独立读取五项卡片计数。
- 备份下载/删除使用 `{ backupId }`；Desktop 下载列表使用 `{ refresh? }`。

## 数据合同

- 翻页时先更新控件目标；成功后保存确认页码；只有当前请求失败才回退。
- 旧请求的成功或失败均不得覆盖新请求；筛选失败不得复活旧筛选结果。
- 额度预测使用 `useRequestState`，保留原共享缓存、刷新动作与关闭过渡帧。
- 昨日范围为部署时区上一自然日 `[start,end)`，不按固定 24 小时推算。
- PostgreSQL 通过现有查询预算聚合，排除已恢复请求和质量检测；Token 沿用完整交付事实。
- 保留现有首页查询缓存，不另设昨日缓存；不得以趋势范围代替独立昨日计数。

## 错误与边界

| 情况 | 行为 |
| --- | --- |
| 翻页失败 | 原列表及确认页码保持一致 |
| 筛选失败 | 第一页空列表，展示本次错误 |
| 弹窗关闭或账号变化 | 旧响应失效，不覆盖新账号 |
| 今日无请求 | 不展示涨跌箭头 |
| 缓存命中率未知 | 不把 null 当作 0 比较 |
| 昨日查询失败 | 返回既有管理端依赖错误，不伪造零值 |

## 示例

正常：今日 2 次、昨日 7 次，今天范围仍显示昨日 7 次。
空数据：今日无请求，隐藏箭头。
错误：第三页加载失败时仍展示第二页内容，却把控件留在第三页。

## 回归

账户列表测试覆盖页码/每页数量失败回退、旧错误、筛选清空；预测测试覆盖缓存和关闭。
管理服务测试覆盖跨日、窄趋势；数据库测试覆盖半开边界、修正回读、质量和恢复排除。
浏览器测试覆盖实际分页组件及桌面、390/320px 布局。

## 不可替代的合同

- TPS 使用 `latencyDetails.upstreamResponseMs`，即同一完成响应的上游时间跨度。
  不使用总耗时、首 Token 差值或引擎时间回退；输出 Token 已包含推理，不重复扣除。
  无有效上游时间时列表显示未知，聚合不加入分位数样本，但原 Token/金额仍按
  完整交付合同统计。旧记录应复用已存观测，不为展示重新请求模型或重写历史。

错误：照搬官方返回结构或插件认证流程。
正确：沿用 NewCPR 的 `DashboardResult.today/yesterday`、认证修订与 cookie 过渡保护。
请求顺序使用有序移除；被动专项计时仍用于本项目诊断，不随官方删除。

## 独立统计卡片

官方 #556：summary、health、performance、cost、diagnostics 独立加载和失败，
切换指标仅查询所属卡片。只有 load 查询并发；保留本地 scheduling 图与 Key × 模型分页。
独立 scheduling 响应未包含请求活动数，空样本保留 null，不能按空 activity 数组补为零。
API、SQL 与缓存键都必须接收完整筛选，不能只传时间与 Provider。
带 metric 的 API 不得双层 flatten 严格查询结构；先取出指标，再用原筛选结构解析。
数字字段复用 query_number，路由测试必须带 Provider、账号、状态码和尝试次数，不能只测空筛选。
本地使用即时查询和恢复小时基线，缓存 2 秒/64 项，不移植不存在的 rollup 代次字段。
同键初始化等待、数据库及费用恢复共用 store 的查询总时限，失败不可缓存成成功。
更换筛选取消并清空旧数据；同筛选翻页失败保留旧页并显示错误。浏览器测试必须考虑
既有 GET 一次瞬时重试，测试信封必须包含 code/message/data，使用当前界面按钮名。

## 三语与管理会话适配

- 范围：v3.30.2 管理端适配；自有动态中文文案尚未全部翻译，不宣称全站覆盖。
- 接口：`setLocale(Locale): Promise<boolean>`；`sessionFetch` 和 Axios 共用会话恢复状态。
  服务端 `RelativeTimeView` 提供 just_now/elapsed/absolute，旧 display 字段保留。
- 合同：先加载简中回退，只有最后一次语言选择生效；数字按语言格式化。
  相对时间用服务端计算结果，未来时间保留部署时区绝对值，不用客户端时间重算。
  登录代次变化后，旧成功响应和旧流帧也必须取消；仅明确 401/40101 才可恢复并重试一次。
  已接收的流式 POST、成功写入、503 不重放。CC Switch 明文只活在待确认状态，关闭/卸载清空。
- 边界：旧语言加载晚到不能覆盖新语言；迟到密钥 reveal 不能重开已卸载页面；未知时间用旧显示。
- 示例：快速点击英文再繁中，英文晚返回仍显示繁中；普通中文功能继续使用；不能把 JSON 对象构建成功当浏览器验收。
- 验证：三语资源、加载竞态、Blob 401、成功旧响应、SSE 旧代次、导入关闭/卸载；
  1440/390/320px 浏览器检查导入、余额、主题、登录语言持久化及使用统计公共筛选位于图表前、时间入口唯一，列表状态仍位于请求明细。
  隐藏的独立筛选输入不删除 URL/钻取参数；已有条件须显示为可移除标签。自定义起止时间一起应用，沿用原本地日界限。
- 正误：不能整体替换本地列表组件丢掉筛选/模板/余额功能；在原组件接入文案和生命周期保护。


### QX query presentation

The common usage toolbar wraps by field width; expanded filters auto-fit rather
than enforcing two columns. Display the effective time range on its button and
apply both custom boundaries together. Individual condition chips remove only
that condition, preserving time, selected accounts and the current view.
Chart and diagnostics placement may change with viewport width; their transforms,
request parameters, ranking and pagination remain unchanged. Full-width diagnostics
keep existing columns and horizontal scrolling where necessary.

## 请求错误来源与独立列配置

- 官方 v3.53.0：请求明细 `recordStatus=error` 可附带
  `errorOrigin=proxy|upstream|connection|unknown`；其他状态携带来源应拒绝。
  来源按已保存事实解释，不能根据客户端状态码或错误文案猜测。
- 列表、总数、分页在同一来源过滤后计算，投影和 SQL 共享分类优先级。
  列表专属的状态/来源不进入图表与汇总；汇总/概览/指标 API 对误带这些字段的请求返回 400，不能静默忽略。公共时间、账号、模型等筛选继续联动。
- 成功/全部共用 `usage-records`，错误使用 `usage-error-records` 保存显示和顺序。
  错误首列固定、不可隐藏；操作右固定。时间窄列显示时间与日期，原完整值作为提示。
- 基础表格仅允许显式固定在左侧且处于该分区末尾的 custom 列按 grow 扩展；
  selection/actions 等固定宽度不受影响。列移动不能跨越左右固定分区。
- 数组操作须兼容继承的 Vue DOM 配置（ES2022 lib）；提高 target 不会自动提高 lib。
  涉及新内建 API 的改动使用强制重新类型检查验证，避免增量缓存漏掉构建兼容问题。
- 保留 NewCPR 原有全部视图、详情入口、分析图表和当前样式。验证实际组件的横向滚动、
  列排序、隐藏、恢复默认与视图隔离，不以单独类名断言替代浏览器检查。
