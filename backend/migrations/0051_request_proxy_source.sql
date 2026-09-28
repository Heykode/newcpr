-- Account request exits are independent of Codex/Excel and preserve native egress.
alter table provider_accounts
    add column request_proxy_source text not null default 'account'
    check (request_proxy_source in ('account', 'mihomo', 'proxy_pool'));
