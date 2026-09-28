-- Historical display metadata; do not change active pause or recovery semantics.
alter table provider_accounts
    add column excel_403_warning_at timestamptz;

update provider_accounts set excel_403_warning_at = excel_auto_disabled_at
where excel_auto_disabled_at is not null and responses_upstream = 'excel';
