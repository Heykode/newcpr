# Database Guidelines

> Database patterns and conventions for this project.

---

## Overview

<!--
Document your project's database conventions here.

Questions to answer:
- What ORM/query library do you use?
- How are migrations managed?
- What are the naming conventions for tables/columns?
- How do you handle transactions?
-->

(To be filled by the team)

## Account Timestamp Invariant

`provider_accounts.updated_at` must not move backwards on an existing row.
Retain the stored value with `greatest(now(), updated_at, ...)` and include
all newly written observation times. The existing database constraint already
ensures the previous value covers retained creation and observation timestamps.
Credential, quota-document and quota-access observations can advance independently;
quota CAS must cover both the document and access observation times.

Do not rewrite observation timestamps, token expiry, reset times or revision
checks merely to satisfy this invariant. Do not increment timestamps artificially
or treat `updated_at` as a unique version. A clock rollback may temporarily keep
the displayed update time unchanged; credential/config revisions remain the
concurrency controls. Verify using isolated PostgreSQL with synthetic future-dated
rows, normal-time writes and stale/concurrent revision attempts, never by changing
the host clock.

---

## Query Patterns

<!-- How should queries be written? Batch operations? -->

(To be filled by the team)

---

## Migrations

<!-- How to create and run migrations -->

(To be filled by the team)

---

## Naming Conventions

<!-- Table names, column names, index names -->

(To be filled by the team)

---

## Common Mistakes

<!-- Database-related mistakes your team has made -->

(To be filled by the team)
