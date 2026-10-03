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
