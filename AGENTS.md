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

## Verified Image Deployment

- Before deployment, fetch `origin/main` and reread this file and
  `deploy/verified-deployment.md`. Do not rely on another window's stale checkout
  or on deployment commands remembered from conversation history.
- Normal application deployment uses `python3 release/deploy.py --profile
  "$HOME/.config/cpr/deploy-production.json" --commit <full-main-commit>`.
  This is a read-only plan. Add `--apply` only when the user authorized deployment.
  The real profile stays outside Git with mode 0600; never put its contents in PRs.
- Reuse only the verified image selected by that command. Do not rebuild on the
  production server, generate a one-off rollout script, skip failed checks, or
  select a different artifact just because a previous deployment was slow.
- Missing, expired, or mismatched artifacts require CI for the exact target
  commit, not an unverified fallback build. If CI is already running, wait for it
  rather than dispatching duplicate runs. Check both CI and Security Scan.
- Only the requested application may be recreated. Preserve configured protected
  services, account identities, network settings, configuration, and graceful
  drain. Keep the old image and backups. Never use `docker compose down` to update.
- The fast path refuses database migration changes and major-version changes.
  Plan those separately; do not force an image rollback across a changed schema.
- Formal multi-platform releases (`release/publish`) are separate from normal
  deployment. Do not invoke the release workflow merely to update one server.
- Report the actual deployed source/merge commits and measured health gap.
  Preparation time is not downtime; do not promise a fixed zero-downtime window.
