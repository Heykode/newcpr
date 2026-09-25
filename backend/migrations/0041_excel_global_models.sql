alter table runtime_settings
    add column excel_default_models text[] not null
        default array['gpt-5.6-sol', 'gpt-6-astra']::text[],
    add constraint runtime_settings_excel_default_models_ck check (
        cardinality(excel_default_models) <= 64
        and array_position(excel_default_models, null) is null
    );

-- Existing lists may be intentional, including the former single-model default.
alter table provider_accounts
    add column excel_models_follow_global boolean not null default false;
alter table provider_accounts
    alter column excel_models_follow_global set default true;
