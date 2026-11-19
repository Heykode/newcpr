-- Maintenance routing is independent from inference routing. Old accounts follow their route.
-- Keep an unavailable reference visible if a proxy is deleted; never silently fall back to direct.
alter table provider_accounts add column credential_proxy_id text;
create index provider_accounts_credential_proxy_id_idx on provider_accounts (credential_proxy_id)
    where credential_proxy_id is not null;
