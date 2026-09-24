-- Manual location remains independent so disabling automatic lookup restores it.
alter table outbound_proxies
    add column auto_location boolean not null default false,
    add column detected_location_json jsonb,
    add column last_location_detection_json jsonb not null default '{"status":"notRequested"}';
