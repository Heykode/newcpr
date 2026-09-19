alter table runtime_settings
    add column turn_state_probe_proxy_id text
    references outbound_proxies(id) on delete restrict;
