alter table provider_accounts
    add column excel_auto_disable_on_403 boolean not null default false;
