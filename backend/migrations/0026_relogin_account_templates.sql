create table account_relogin_templates (
    id text primary key,
    revision bigint not null check (revision > 0),
    name text not null,
    config jsonb not null
);

create unique index account_relogin_templates_name
    on account_relogin_templates (lower(name));
