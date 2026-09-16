-- Keep legacy combined selections intact; independent choices are explicit.
alter table provider_outbound_user_agents
    add column tls_profile text,
    add column session_policy text,
    drop constraint provider_outbound_user_agents_mode_check,
    drop constraint provider_outbound_user_agent_selection;

alter table provider_outbound_user_agents
    add constraint provider_outbound_user_agents_mode_check
        check (mode in ('default', 'custom', 'qx-compatible', 'independent')),
    add constraint provider_outbound_user_agent_selection check (
        (mode = 'default' and custom_user_agent is null)
        or (mode = 'custom' and custom_user_agent is not null
            and octet_length(custom_user_agent) between 1 and 512
            and custom_user_agent !~ '[[:cntrl:]]')
        or (mode in ('qx-compatible', 'independent') and (
            custom_user_agent is null
            or (octet_length(custom_user_agent) between 1 and 512
                and btrim(custom_user_agent) <> ''
                and custom_user_agent !~ '[[:cntrl:]]')
        ))
    ),
    add constraint provider_outbound_user_agent_independent_choices check (
        (mode <> 'independent' and tls_profile is null and session_policy is null)
        or (mode = 'independent'
            and tls_profile is not null and tls_profile in ('cpr', 'qx-compatible')
            and session_policy is not null and session_policy in ('native', 'qx-compatible'))
    );
