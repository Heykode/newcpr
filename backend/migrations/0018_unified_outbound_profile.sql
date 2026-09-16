-- Preserve the original selection before removing the retired transport modes.
alter table provider_outbound_user_agents
    add column legacy_selection jsonb;

update provider_outbound_user_agents
set legacy_selection = jsonb_build_object(
    'mode', mode,
    'custom_user_agent', custom_user_agent,
    'tls_profile', tls_profile,
    'session_policy', session_policy,
    'updated_at', updated_at
);

alter table provider_outbound_user_agents
    drop constraint provider_outbound_user_agents_mode_check,
    drop constraint provider_outbound_user_agent_selection,
    drop constraint provider_outbound_user_agent_independent_choices;

-- The old QX default was pinned, not the auto-updated Desktop default.
update provider_outbound_user_agents
set custom_user_agent = 'codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color'
where mode = 'qx-compatible'
  and (custom_user_agent is null or btrim(custom_user_agent) = '');

update provider_outbound_user_agents
set mode = case when custom_user_agent is null then 'default' else 'custom' end;

alter table provider_outbound_user_agents
    drop column tls_profile,
    drop column session_policy,
    add constraint provider_outbound_user_agents_mode_check
        check (mode in ('default', 'custom')),
    add constraint provider_outbound_user_agent_selection check (
        (mode = 'default' and custom_user_agent is null)
        or (mode = 'custom' and custom_user_agent is not null
            and octet_length(custom_user_agent) between 1 and 512
            and btrim(custom_user_agent) <> ''
            and custom_user_agent !~ '[[:cntrl:]]')
    );
