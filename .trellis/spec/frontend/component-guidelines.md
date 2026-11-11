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

<!-- How styles are applied (CSS modules, styled-components, Tailwind, etc.) -->

(To be filled by the team)

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

### 界面文案与校验时机

- 沿用原页面、基础组件和操作位置。标题、按钮只写名称或动作，不重复字段值、
  状态和已清楚的交互；详细规则放已有 Popover，关键错误与不可逆后果就近展示。
- 自编 UI 文案用短语，不用句号、分号；URL、版本、数值、原始日志和用户输入不改。
- Tooltip 放短名称，Popover 放多段说明或链接，帮助入口兼容键盘与触屏。
- 官方 ZPopover 的 `reference` 插槽接入本地 BasePopover 时使用 `trigger`；帮助内容保留命名 dialog，
  图标放既有 FormItem `label-extra`，按现有 BaseIconButton 的 `label`/`size` 接口适配。浏览器须确认入口真实出现且能打开，不能仅凭类型检查。
- 新增表单项不在尚未编辑时显示必填错误；失焦或提交后提示，提交始终完整验证。
  动态规则使用稳定身份，删除前项不能把后项的输入状态移到其他规则。
- API Key 安装 ID 默认空，生成只填草稿，保存后发送；清空删除该 Key 覆盖。
  代理测试结果放选择器旁，出口 IP 放详情，不新增说明卡片。

### 主题与共享侧栏（主题对齐官方 UI v0.9.1）

- `createTheme({ name, preset, colorId, primaryColor, customization })` 统一派生。
  `themeProfiles` 是完整组合，`themePresets` 只控制表面高光、渐变和阴影。
  材质不提供 customization，不改变配色、圆角、字号和尺寸；显式 customization 独立生效。
  无自定义时柔光控件圆角为 8px、卡片 18px、侧栏 0；明暗模式一致。
  持久化键 `codex-proxy-rs-theme` 不变，无 `themePreset` 的旧配置补 `default`。
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
