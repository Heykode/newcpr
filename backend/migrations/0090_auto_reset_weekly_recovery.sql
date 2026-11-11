-- Account-scoped automation shares the existing durable consumption ledger.
alter table account_auto_reset_policies
    add column awaiting_weekly_recovery_since timestamptz;
