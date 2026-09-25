-- A new switch, never inferred from the legacy managed-State configuration.
alter table provider_accounts
    add column responses_upstream text not null default 'codex',
    add constraint provider_accounts_responses_upstream_ck check (
        responses_upstream in ('codex', 'excel')
        and (
            responses_upstream = 'codex'
            or (provider_kind = 'openai' and authentication_kind = 'oauth')
        )
    );
