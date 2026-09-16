# Directory Structure

> How backend code is organized in this project.

---

## Overview

<!--
Document your project's backend directory structure here.

Questions to answer:
- How are modules/packages organized?
- Where does business logic live?
- Where are API endpoints defined?
- How are utilities and helpers organized?
-->

(To be filled by the team)

---

## Directory Layout

```
<!-- Replace with your actual structure -->
src/
├── ...
└── ...
```

---

## Module Organization

<!-- How should new features/modules be organized? -->

(To be filled by the team)

### Verified Rust module layout

- A leaf `name.rs` must not coexist with child files under `name/`.
  When adding child modules, move the owner to `name/mod.rs` and preserve
  its existing module path. Move an existing matching integration-test owner
  from `tests/name.rs` to `tests/name/mod.rs` as well.
- Do not relax the architecture test or add layout exemptions to accommodate
  a new feature. A passing Rust compiler is not a passing project layout check.
- After creating or moving modules, run the gateway architecture regressions,
  not only the affected provider's tests:
  `cargo test --locked -p codex-proxy-rs --test main architecture`.
- The executable contracts are in `backend/apps/gateway/tests/architecture.rs`:
  `workspace_modules_follow_conventional_file_layout` and
  `integration_tests_mirror_production_module_tree`.

---

## Naming Conventions

<!-- File and folder naming rules -->

(To be filled by the team)

---

## Examples

<!-- Link to well-organized modules as examples -->

(To be filled by the team)
