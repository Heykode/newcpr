create table quality_rule_templates (
    id text primary key,
    revision bigint not null default 1,
    name text not null check (char_length(btrim(name)) between 1 and 128),
    config jsonb not null check (coalesce(config->>'accountId', '') = ''),
    updated_at timestamptz not null default now()
);

-- Keep provenance after template edits/deletion; applied rules are independent.
alter table quality_rules add column source_template jsonb;
