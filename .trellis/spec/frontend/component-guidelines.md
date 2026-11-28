# Component Guidelines

> How components are built in this project.

---

## Overview

<!--
Document your project's component conventions here.

Questions to answer:
- What component patterns do you use?
- How are props defined?
- How do you handle composition?
- What accessibility standards apply?
-->

(To be filled by the team)

---

## Component Structure

<!-- Standard structure of a component file -->

(To be filled by the team)

---

## Props Conventions

<!-- How props should be defined and typed -->

(To be filled by the team)

---

## Styling Patterns

- QX presentation uses the existing theme compiler and shared Base components. The
  `qianxing` color profile supplies warm surfaces and terracotta accents; explicit
  seed and component/token overrides still win, and existing nondefault profiles remain.
- `themeDesignVersion` migrates only the untouched former default once. Do not reset
  customized themes on initialization or when the user later selects an old profile.
- Bundle QX Display with its OFL license; use it for headings and system fonts for
  body text. Preserve semantic chart colors, table column keys and density overrides.
- QX is a layout and interaction design, not only a palette. Use outlined fields,
  compact navigation, a top bar with the mobile menu, restrained panel geometry,
  flexible field widths and visible removable filter conditions. The menu must not
  float over content. Keep route-guard cancellation, mobile focus trapping and
  focus restoration intact.
- Form actions wrap when their minimum readable width does not fit. Avoid fixed
  two-column filter layouts across unrelated viewport sizes or squeezing a long
  date range behind ellipsis. Preserve customized theme tokens and dimensions.
- Shared visual work must not rewrite request health, hotspot ranking, analytics SQL,
  queue/affinity rules, sorting, pagination or account eligibility. Keep presentation
  tests aligned with approved layouts instead of restoring obsolete UI restrictions.

---

## Accessibility

<!-- A11y requirements and patterns -->

(To be filled by the team)

---

## Common Mistakes

<!-- Component-related mistakes your team has made -->

(To be filled by the team)

### Asynchronously Persisted Switches

- A native checkbox can retain its clicked state when its controlled value is
  unchanged after a rejected API save. Do not assume an unchanged model prop
  restores the DOM automatically.
- Keep a local displayed value for the pending edit and reconcile it with
  confirmed settings after the save completes. Disable repeated changes while
  saving; retain a visible failure state and allow retry.
- Follow `DetailedCaptureControl.vue` for this request-local pattern. Do not
  change the shared `BaseSwitch` merely to implement one feature's persistence.
- Browser regression coverage must assert the actual checkbox state after
  rejection, then a successful retry; API-payload assertions alone are not enough.

### Modal Exit Lifecycle

- `BaseModal` emits `afterLeave` after its real transition. Preserve visible
  content, titles and confirmation counts until that event, rather than replacing
  them with empty defaults when the open model becomes false.
- Closing still cancels requests and invalidates stale responses immediately.
  Delayed visual cleanup must not allow a closed request to update another modal.
- Clear editable drafts, selected targets and plaintext keys on `afterLeave`,
  only if that modal is still closed. A late callback cannot erase a rapid reopen.
- Multi-root modal wrappers must explicitly forward each modal's leave event;
  single-root wrappers may forward undeclared listeners to their `BaseModal` root.
- Verify non-reduced-motion exit frames, rapid reopen and sensitive-content
  removal in desktop/mobile browsers, plus composable cleanup regressions.
- `BasePopover.closed` fires after the panel's leave transition completes, only
  while still closed. Consumers restore focus after that event, without taking
  focus away from another control selected by the user. A disabled cleanup trigger
  returns focus to its caller's available fallback. Cover normal/reduced motion,
  cancellation, outside clicks and rapid reopen in browser regressions.
- Hover/click help tests must verify the panel is open at the moment Escape is
  sent. Viewport changes can dismiss hover content; avoid mixing delayed focus
  opening with a click that toggles it closed.

### 界面文案与校验时机

- 复用既有业务组件和处理函数，布局与操作位置以当前批准设计为准。标题、按钮只写名称或动作，不重复字段值、
  状态和已清楚的交互；详细规则放已有 Popover，关键错误与不可逆后果就近展示。
- 自编 UI 文案用短语，不用句号、分号；URL、版本、数值、原始日志和用户输入不改。
- Tooltip 放短名称，Popover 放多段说明或链接，帮助入口兼容键盘与触屏。
- 官方 ZPopover 的 `reference` 插槽接入本地 BasePopover 时使用 `trigger`；帮助内容保留命名 dialog，
  图标放既有 FormItem `label-extra`，按现有 BaseIconButton 的 `label`/`size` 接口适配。浏览器须确认入口真实出现且能打开，不能仅凭类型检查。
- 账号、API Key、代理行菜单使用官方 v0.12 的 `BaseDropdown` 键盘逻辑和共享 `RowActionMenu`。
  方向键、Home/End 跳过禁用项；Enter/Space 激活；Escape/Tab 关闭；选择后恢复触发按钮焦点。
  `BaseMenuItem` 只在 Dropdown 内设置 `menuitem` 和 `tabindex=-1`，其他消费者保持普通按钮。
  浏览器测试按 `menu/menuitem` 定位，不为旧按钮断言去掉官方菜单语义。
- `BaseIconButton.pressed` 未提供时省略 `aria-pressed`，显式 false 必须保留为 `aria-pressed="false"`。
- 新增表单项不在尚未编辑时显示必填错误；失焦或提交后提示，提交始终完整验证。
  动态规则使用稳定身份，删除前项不能把后项的输入状态移到其他规则。
- API Key 安装 ID 默认空，生成只填草稿，保存后发送；清空删除该 Key 覆盖。
  代理测试结果放选择器旁，出口 IP 放详情，不新增说明卡片。

### 主题与共享侧栏（主题对齐官方 UI 68458e62e）

- `createTheme({ name, preset, colorId, primaryColor, customization })` 统一派生。
  `themeProfiles` 提供尺寸/材质组合，`resolveThemeProfile` 保留现有功能色和中性色，
  未编辑背景/文字使用明暗默认值。主色不连带覆盖信息/链接色。`themePresets` 只控制表面高光、渐变和阴影。
  材质不提供 customization，不改变配色、圆角、字号和尺寸；显式 customization 独立生效。
  无自定义时常规配置控件圆角为 8px、卡片 18px；QX 为 6px 控件和 10px 卡片，侧栏为 0。明暗模式一致，显式尺寸覆盖优先。
  持久化键 `codex-proxy-rs-theme` 不变，无 `themePreset` 的旧配置补 `default`。
- 预设底色仅参与表面派生，不写回 seed；显式 `colorBgBase` 优先。输入标签底色从
  输入背景合成为实色后派生，包含用户的 `--cp-input-bg` 覆盖。
- 本地 CSS Token 与直接覆盖键仍为 `--cp-*`。移植官方无前缀 Token 消费者时，
  同步调整过滤、预览和显示；派生变量列表不得因错误前缀变成空白。
- `BaseSidebar` 是公共布局；`AppSidebar` 承担本地菜单与路由。
  `defaultActive` 和 `updateActiveIndex` 必须对应真实路由，拒绝跳转不高亮目标。
  手机点击菜单先收起遮罩和焦点锁，再等待 route guard，避免离页确认框被遮挡。
- `BasePopover` 的 `enterable=false` 禁止鼠标驻留，不抹掉键盘 focusin 和离开清理；
  `role` 只接受 tooltip/dialog。折叠侧栏使用 tooltip，分组 Popover 保留触屏点击。
- 浏览器验收包含四预设、两材质、明暗、旧存储/保存刷新、自定义覆盖、被取消的导航、
  手机焦点与真实多分组几何布局。复用 `theme-preview.mjs` 和 `upstream-v3330.mjs`，
  单次场景扩展放仓库外；仅类型通过不能证明遮挡和重叠不存在。

### 输入控件（官方 UI v0.11.0）

- Input、Textarea、Select、InputTag、NumberInput 共用 `controls/input-styles.ts` 的行高、
  内缩、禁用和错误状态。尺寸接现有 `sm/md/lg`；NumberInput 默认仍为 `sm`。
  输入/标签/选择器的默认宽度放在组件层，让调用方宽度类可以覆盖。
- `BaseSelect` 保留官方字符串/数字泛型、单选/多选、搜索/remote、清空、折叠标签、
  键盘导航和禁用选项。单选清空返回 `undefined`，页面接入时保留其空筛选或不接受清空的业务合同，
  不把共享控件缩成仅字符串、不支持清空的实现。
- `BaseInput` 的自动填充 CSS 动画和监听名称必须一致；专属 `cp-input__field` 选择器的
  动画样式不使用 scoped 名称改写。验证浏览器真实动画事件同步 model，不只派发 input。
- `BaseInputTag` 支持回车/空格提交、失焦保存、IME、退格删除、最大数量、只读/禁用、
  拖拽排序及折叠弹层。溢出标签的 Popover 使用本地 `trigger` 插槽和 `teleported=false`，
  内部焦点移动不误提交；`BasePopover` 默认仍 Teleport。
- 账号分组继续使用既有多文字标签布局，不能因为输入控件标签更新改成图标叠加。
- 浏览器检查实际设置保存重读、标签及选择器键盘/手机交互；旧测试辅助导入和
  schema 要同步，不以还原旧组件行为通过检查。

### PWA window controls overlay

Follow the official display-mode/geometrychange flow: a draggable blank titlebar,
settled visual viewport height, body inset, full-height page slot, mobile sidebar
and toast offset. Use local cp theme tokens and update theme-color on theme commit.
Normal browser tabs have no inset. Check actual browser geometry at short and mobile
viewports after resize/zoom; CSS class assertions alone cannot establish no overlap.
Quota-reset settings share the three-window input component with Smart, but each
strategy keeps its own object and reset defaults. Keep the existing inline layout.

### v3.54.1 shared display rules

- Request country has its own card with four policy descriptions and an explicit
  Codex OAuth scope. HelpPopover supports structured default-slot content while
  keeping the plain content prop for existing consumers. Preserve keyboard/touch access.
- Account and group forms share the model access editor; blocklist wording must
  not imply account-only behavior. Keep the 15 Codex account plan filters separate
  from upstream API-key account types.
- Shared usage tables always pass the original timestamp to UsageTimeCell and
  define the 128px time width in shared columns, including dashboard callers.
- Cost chart legend, lines and tooltips derive from one series definition. Empty
  activity can fill zero; active buckets with unknown prices remain null. Missing
  comparison estimates do not generate tooltip rows.
- Proxy create/test displays and completion notifications share batch summaries.
  Pending/running/skipped are not completed; location detection is independent of
  connectivity. Preserve optional location access and prioritize connection errors
  over location messages. Reuse lifecycle fencing and stop behavior.
- Local BaseTable uses sticky th backgrounds, not the upstream thead pseudo-element.
  Do not replace the local library merely to copy a submodule bump. Check actual
  WebKit and Chromium scrolling, fixed columns and expanded rows before parity claims.
