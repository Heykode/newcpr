alter table provider_accounts
    add column excel_models text[] not null default array['gpt-5.6-sol']::text[],
    add constraint provider_accounts_excel_models_ck check (
        cardinality(excel_models) <= 64
        and array_position(excel_models, null) is null
    );
