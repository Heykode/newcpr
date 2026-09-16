<!-- TRELLIS:START -->
# Trellis Instructions

These instructions are for AI assistants working in this project.

This project is managed by Trellis. The working knowledge you need lives under `.trellis/`:

- `.trellis/workflow.md` — development phases, when to create tasks, skill routing
- `.trellis/spec/` — package- and layer-scoped coding guidelines (read before writing code in a given layer)
- `.trellis/workspace/` — per-developer journals and session traces
- `.trellis/tasks/` — active and archived tasks (PRDs, research, jsonl context)

If a Trellis command is available on your platform (e.g. `/trellis:finish-work`, `/trellis:continue`), prefer it over manual steps. Not every platform exposes every command.

If you're using Codex or another agent-capable tool, additional project-scoped helpers may live in:
- `.agents/skills/` — reusable Trellis skills
- `.codex/agents/` — optional custom subagents

Managed by Trellis. Edits outside this block are preserved; edits inside may be overwritten by a future `trellis update`.

<!-- TRELLIS:END -->

## Privacy Before Writing

Read `tools/privacy/README.md` before writing operational notes, fixtures,
configuration examples, commit messages, PRs or release descriptions.
Never write real server endpoints, account identifiers, credentials, private keys,
personal home paths or raw production traffic into repository files or Git metadata.
Keep raw evidence outside the checkout and use sanitized or synthetic examples.
Trellis tasks and journals are local-only, not tracked public source.
Run the privacy checks on exact staged content and the complete publication history.
Do not bypass hooks or auto-generate exemptions to obtain a pass.
Report only sanitized paths, line numbers and rules, never matching private values.
Install hooks in each new clone before committing. Repository creation, push,
release and deployment require the user's applicable authorization.
