-- Empty profiles preserve each selected account's existing identity.
alter table client_api_keys
    add column provider_request_profiles_json jsonb not null default '{}'::jsonb,
    add constraint client_api_keys_request_profiles_object
        check (jsonb_typeof(provider_request_profiles_json) = 'object');
