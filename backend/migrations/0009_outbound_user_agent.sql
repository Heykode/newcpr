-- Outbound identity settings are intentionally outside runtime_settings replacement.
create table provider_outbound_user_agents (
    provider_kind text primary key,
    mode text not null check (mode in ('default', 'custom')),
    custom_user_agent text,
    updated_at timestamptz not null default now(),
    constraint provider_outbound_user_agent_selection check (
        (mode = 'default' and custom_user_agent is null)
        or (mode = 'custom' and custom_user_agent is not null
            and octet_length(custom_user_agent) between 1 and 512
            and custom_user_agent !~ '[[:cntrl:]]')
    )
);
