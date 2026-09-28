-- Explicit, lossy Excel compatibility option; existing accounts stay unchanged.
alter table provider_accounts
    add column excel_ignore_encrypted_content boolean not null default false;
