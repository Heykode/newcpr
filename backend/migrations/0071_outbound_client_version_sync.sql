-- Existing custom profiles remain pinned until the administrator opts in.
alter table provider_outbound_user_agents
    drop constraint provider_outbound_user_agents_mode_check,
    drop constraint provider_outbound_user_agent_selection,
    add constraint provider_outbound_user_agents_mode_check
        check (mode in ('default', 'custom', 'custom-auto-version')),
    add constraint provider_outbound_user_agent_selection check (
        (mode = 'default' and custom_user_agent is null)
        or (mode in ('custom', 'custom-auto-version') and custom_user_agent is not null
            and octet_length(custom_user_agent) between 1 and 512
            and btrim(custom_user_agent) <> ''
            and custom_user_agent !~ '[[:cntrl:]]')
    );
