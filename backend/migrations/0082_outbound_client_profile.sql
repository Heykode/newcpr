-- Existing selections and exact UA text remain unchanged.
alter table provider_outbound_user_agents
    add column configuration_json jsonb,
    drop constraint provider_outbound_user_agents_mode_check,
    drop constraint provider_outbound_user_agent_selection,
    add constraint provider_outbound_user_agents_mode_check
        check (mode in ('default', 'custom', 'custom-auto-version', 'configuration')),
    add constraint provider_outbound_user_agent_selection check (
        (mode = 'default' and custom_user_agent is null and configuration_json is null)
        or (mode in ('custom', 'custom-auto-version') and configuration_json is null
            and custom_user_agent is not null
            and octet_length(custom_user_agent) between 1 and 512
            and btrim(custom_user_agent) <> '' and custom_user_agent !~ '[[:cntrl:]]')
        or (mode = 'configuration' and custom_user_agent is null
            and configuration_json is not null and jsonb_typeof(configuration_json) = 'object'
            and octet_length(configuration_json::text) <= 16384)
    );
