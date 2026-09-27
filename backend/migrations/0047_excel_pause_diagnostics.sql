-- Diagnostic metadata only; credentials, identity and routing defaults stay unchanged.
alter table provider_accounts
    add column excel_auto_disabled_at timestamptz;
