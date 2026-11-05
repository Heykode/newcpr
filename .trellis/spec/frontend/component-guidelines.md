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
- 新增表单项不在尚未编辑时显示必填错误；失焦或提交后提示，提交始终完整验证。
  动态规则使用稳定身份，删除前项不能把后项的输入状态移到其他规则。
- API Key 安装 ID 默认空，生成只填草稿，保存后发送；清空删除该 Key 覆盖。
  代理测试结果放选择器旁，出口 IP 放详情，不新增说明卡片。
