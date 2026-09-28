alter table provider_accounts
    add column excel_403_action text not null default 'none'
        check (excel_403_action in ('none', 'pause_account', 'disable_excel')),
    add column excel_mode_disabled_at timestamptz;

update provider_accounts set excel_403_action = 'pause_account'
where excel_auto_disable_on_403;

update provider_accounts set excel_mode_disabled_at = excel_auto_disabled_at
where responses_upstream = 'codex' and excel_auto_disabled_at is not null;

-- Existing choices and historic usage are not rewritten.
alter table provider_accounts alter column excel_cache_creation_as_input set default true;
