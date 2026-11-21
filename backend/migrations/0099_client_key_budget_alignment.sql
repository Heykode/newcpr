-- Version budget edits independently of usage settlement; retain the existing charge ledger.
alter table client_key_budget_windows
    add column revision bigint not null default 0 check (revision >= 0),
    add column last_operation_request jsonb;
