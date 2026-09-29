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
