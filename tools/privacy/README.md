# Privacy Checks

The checks are read-only. They never commit, push, export, delete source files,
create repositories, or change visibility. Installation is a separate local action.

## Before Writing

Keep raw operations records, production configuration, endpoint lists, screenshots
and audit evidence outside the repository. Write sanitized summaries only.
Keep reusable Trellis specs and scripts; task records and journals are local-only
in the future public repository. Do not remove the private development history.

## Commands

Use a locally verified Gitleaks executable compatible with the flags in `check.py`
(tested with v8.30.1) and Python 3.9 or later:

```sh
export PRIVACY_GITLEAKS=/path/to/verified/gitleaks
python3 tools/privacy/check.py staged
python3 tools/privacy/check.py tree HEAD
python3 tools/privacy/check.py history HEAD
python3 tools/privacy/check.py text /path/to/local/pr-draft.md
python3 -m unittest discover -s tools/privacy -p 'test_*.py' -v
```

`staged` reads index blobs, not working-tree replacements. `history` checks every
reachable commit, including earlier versions deleted later, commit identity,
messages and nested annotated tags' metadata. `pre-push` also checks ref names.
Shallow repositories cannot pass a history check. `tree` checks only the requested snapshot,
not its history. Drafts must be checked before posting PRs, comments or Releases.
Missing Gitleaks, binary content, oversized files and unsupported Git entries fail
closed. Matching values are never printed. Gitleaks inline allow comments and
candidate ignore/config files cannot suppress the scan.

Use the exact reviewed Gitleaks download/checksum process supplied by the tool's
official releases; this repository does not download or execute binaries automatically.

## Private Endpoint List

For exact internal aliases/domains not covered by generic infrastructure rules,
create a private JSON file outside the checkout and set `PRIVACY_BLOCKLIST` to it.
For persistent local hooks, `git config --local privacy.blocklistPath /path/to/private/rules.json`
can be used instead; the environment variable takes precedence.
Its only field is `literals`, an array of non-empty strings of at least four characters.
This file must never be committed or copied into a publication bundle.
No original private endpoint is stored in the repository's policy.

Generic checks detect personal home paths, authenticated URLs, private-key
markers, infrastructure hostnames and non-example IP addresses. Loopback and
documentation addresses are allowed. Unfamiliar real-looking addresses and
synthetic credentials may require review; do not disable scanning entire test
directories or blindly accept a generated baseline.

## Local Hook Installation

Only install after coordinating with the owner of that checkout:

```sh
python3 tools/privacy/install.py --gitleaks "$PRIVACY_GITLEAKS"
```

Installation backs up local Git configuration inside `.git/privacy-hook-backups/`.
It refuses to overwrite custom hooks or implicitly change shared worktrees.
It does not change Git author identity. Set a reviewed public identity separately;
the policy accepts GitHub noreply email domains, not local-machine emails.
Existing commits are not changed when Git identity configuration changes.

An already published commit identity that cannot be rewritten may be listed in
`policy.json` under `reviewed_commit_identities` by its complete object ID and a
non-empty review reason. This exception suppresses only the identity-domain finding
for that exact commit. Its message and every reachable file remain fully scanned;
later commits, abbreviated IDs, duplicate entries and malformed records fail closed.
Never generate this list from scanner output or use it for an unpublished commit.

Each clone needs installation. Hooks can be bypassed, so also use a reviewed,
protected CI check and pre-upload publication checks. CI runs after upload and
does not prevent the initial exposure. GitHub push protection is supplementary.
No scanner can reliably recognize every private fact in free-form prose.
The AI instructions reduce accidental writes but are not a filesystem sandbox:
hooks stop commits and pushes, not a text editor from creating a local file.
Encoded data, arbitrary aliases, names and free-form internal records still need
manual review. Never put the private blocklist into a public CI log or workflow.
Changes to the checker, hooks and policy require independent review; a modified
checker cannot be trusted merely because it reports success on itself.

## Publication Gate

The public path allowlist is in `policy.json`; runtime secrets and private
records are rejected independently even under an allowed source directory.
This checker is not an exporter. After the user confirms migration, separately
review the latest code and uncommitted changes, create a fresh sanitized history,
and verify all exported paths and metadata before any upload.

Repository creation, transfer and publication require separate user authorization.

## Reviewed Synthetic Examples

`reviewed-examples.json` contains individually reviewed source fixtures and parser
false positives, not credentials. Each exception is tied to a path, rule, line
number, line SHA-256 and entire file SHA-256. A changed file requires fresh review.
Private blocklist hits and private-key markers cannot be exempted. Never regenerate
this list automatically from a failed scan or exempt a complete test directory.
The original business fixtures are preserved to avoid changing their assertions.
