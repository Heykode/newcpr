alter table quality_rules add column recovery jsonb not null default '{}';
alter table quality_rules add column last_action text;
alter table quality_runs add column action text;
alter table provider_accounts add column quality_pause_owner text
    references quality_rules(id) on delete set null;

-- Changes to the actual identity or independent disable reason invalidate ownership.
create function invalidate_quality_pause() returns trigger language plpgsql as $$
begin
    if new.enabled
       or new.upstream_user_id is distinct from old.upstream_user_id
       or new.upstream_account_id is distinct from old.upstream_account_id
       or new.excel_auto_disabled_at is distinct from old.excel_auto_disabled_at then
        new.quality_pause_owner := null;
    end if;
    return new;
end;
$$;
create trigger invalidate_quality_pause before update of enabled, upstream_user_id,
    upstream_account_id, excel_auto_disabled_at on provider_accounts
    for each row execute function invalidate_quality_pause();
