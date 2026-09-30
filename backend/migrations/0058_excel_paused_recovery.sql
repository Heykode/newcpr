create table account_excel_recovery (
    account_id text primary key references provider_accounts(id) on delete cascade,
    enabled boolean not null default false,
    interval_minutes integer not null default 60 check (interval_minutes between 1 and 10080),
    generation bigint not null default 1,
    next_probe_at timestamptz not null default (now() + interval '60 minutes'),
    lease_id text,
    lease_until timestamptz,
    last_probe_at timestamptz,
    last_result text,
    last_model text,
    failed_model text,
    recovered_at timestamptz
);
create index account_excel_recovery_due on account_excel_recovery(next_probe_at)
    where enabled;

-- Slots survive account deletion until cancellation has drained the request.
create table excel_recovery_slots (
    id integer primary key check(id between 1 and 3),
    lease_id text,
    lease_until timestamptz
);
insert into excel_recovery_slots(id) values (1),(2),(3);

-- Only administrator/credential/route changes touch these columns; usage polling
-- does not invalidate a claim. An explicit repeated pause also fences old work.
create function invalidate_excel_recovery() returns trigger language plpgsql as $$
begin
    update account_excel_recovery set generation=generation+1,
        recovered_at=case when not new.enabled or new.responses_upstream<>'excel'
            or new.credential_revision<>old.credential_revision then null else recovered_at end,
        next_probe_at=now()+make_interval(mins=>interval_minutes)
    where account_id=new.id;
    return new;
end;
$$;
create trigger invalidate_excel_recovery after update of enabled, credential_revision,
    upstream_user_id, upstream_account_id, responses_upstream, excel_models,
    excel_models_follow_global, excel_403_action, outbound_proxy_id, request_proxy_source,
    model_access_json, excel_auto_disabled_at, excel_mode_disabled_at
    on provider_accounts for each row execute function invalidate_excel_recovery();

-- Account enable/disable publications only bump config_revision. They must not
-- cancel other accounts' successful probes. Real runtime setting changes do.
create function invalidate_excel_recovery_runtime() returns trigger language plpgsql as $$
begin
    update account_excel_recovery set generation=generation+1;
    return new;
end;
$$;
create trigger invalidate_excel_recovery_runtime after update on runtime_settings
    for each row when (
        (to_jsonb(old)-array['config_revision','updated_at']) is distinct from
        (to_jsonb(new)-array['config_revision','updated_at'])
    ) execute function invalidate_excel_recovery_runtime();
