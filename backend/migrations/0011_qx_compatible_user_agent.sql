-- Keep the existing setting owner and preserve explicit default/custom selections.
alter table provider_outbound_user_agents
    drop constraint provider_outbound_user_agents_mode_check,
    drop constraint provider_outbound_user_agent_selection;

alter table provider_outbound_user_agents
    add constraint provider_outbound_user_agents_mode_check
        check (mode in ('default', 'custom', 'qx-compatible')),
    add constraint provider_outbound_user_agent_selection check (
        (mode = 'default' and custom_user_agent is null)
        or (mode = 'custom' and custom_user_agent is not null
            and octet_length(custom_user_agent) between 1 and 512
            and custom_user_agent !~ '[[:cntrl:]]')
        or (mode = 'qx-compatible' and (
            custom_user_agent is null
            or (octet_length(custom_user_agent) between 1 and 512
                and btrim(custom_user_agent) <> ''
                and custom_user_agent !~ '[[:cntrl:]]')
        ))
    );
