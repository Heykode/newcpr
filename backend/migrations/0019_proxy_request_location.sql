alter table outbound_proxies
    add column request_location_json jsonb,
    add constraint outbound_proxies_request_location_object
        check (request_location_json is null or jsonb_typeof(request_location_json) = 'object');
