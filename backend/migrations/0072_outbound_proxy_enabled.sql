-- Preserve existing proxy bindings and default all saved proxies to enabled.
alter table outbound_proxies add column enabled boolean not null default true;
