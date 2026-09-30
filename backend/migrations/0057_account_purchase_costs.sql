-- Purchase accounting is admin-only; account deletion and log retention do not erase it.
create table account_purchase_identities (
    id bigint generated always as identity primary key,
    provider_kind text not null,
    upstream_user_id text not null check (upstream_user_id <> ''),
    upstream_account_id text not null check (upstream_account_id <> ''),
    monthly_cost_cny numeric(20,10) check (monthly_cost_cny >= 0),
    cycle_anchor date,
    history_complete_from timestamptz,
    unique (provider_kind, upstream_user_id, upstream_account_id)
);

create table account_purchase_bindings (
    provider_account_ref text primary key,
    identity_id bigint not null references account_purchase_identities(id)
);
create index account_purchase_bindings_identity on account_purchase_bindings(identity_id);

create table account_purchase_daily_usage (
    identity_id bigint not null references account_purchase_identities(id),
    usage_date date not null,
    amount_usd numeric(40,10) not null check (amount_usd >= 0),
    primary key (identity_id, usage_date)
);

create function bind_account_purchase_identity() returns trigger language plpgsql as $$
declare
    resolved_id bigint;
    previous_id bigint;
begin
    select identity_id into previous_id from account_purchase_bindings where provider_account_ref = NEW.id;
    if coalesce(NEW.upstream_user_id, '') = '' or coalesce(NEW.upstream_account_id, '') = '' then
        delete from account_purchase_bindings where provider_account_ref = NEW.id;
        return null;
    end if;
    insert into account_purchase_identities (provider_kind, upstream_user_id, upstream_account_id)
    values (NEW.provider_kind, NEW.upstream_user_id, NEW.upstream_account_id)
    on conflict (provider_kind, upstream_user_id, upstream_account_id) do nothing;
    select id into resolved_id from account_purchase_identities
    where provider_kind = NEW.provider_kind and upstream_user_id = NEW.upstream_user_id
      and upstream_account_id = NEW.upstream_account_id;
    -- Costs incurred before a principal was known cannot be silently presented as complete.
    if previous_id is null and exists (select 1 from account_cumulative_costs
        where provider_account_ref = NEW.id and currency = 'USD' and amount > 0) then
        update account_purchase_identities set history_complete_from = now() where id = resolved_id;
    end if;
    insert into account_purchase_bindings (provider_account_ref, identity_id)
    values (NEW.id, resolved_id)
    on conflict (provider_account_ref) do update set identity_id = excluded.identity_id;
    return null;
end
$$;

lock table provider_accounts in share row exclusive mode;
lock table model_requests in share row exclusive mode;
lock table account_cumulative_cost_entries in share row exclusive mode;

insert into account_purchase_identities (provider_kind, upstream_user_id, upstream_account_id)
select distinct provider_kind, upstream_user_id, upstream_account_id from provider_accounts
where coalesce(upstream_user_id, '') <> '' and coalesce(upstream_account_id, '') <> '';
insert into account_purchase_bindings (provider_account_ref, identity_id)
select a.id, p.id from provider_accounts a join account_purchase_identities p
on (p.provider_kind, p.upstream_user_id, p.upstream_account_id)
 = (a.provider_kind, a.upstream_user_id, a.upstream_account_id);

create trigger provider_accounts_purchase_insert after insert on provider_accounts
for each row execute function bind_account_purchase_identity();
create trigger provider_accounts_purchase_identity_update
after update of provider_kind, upstream_user_id, upstream_account_id on provider_accounts
for each row when ((OLD.provider_kind, OLD.upstream_user_id, OLD.upstream_account_id)
    is distinct from (NEW.provider_kind, NEW.upstream_user_id, NEW.upstream_account_id))
execute function bind_account_purchase_identity();

-- Freeze identity when a request selects its account, before credentials may change.
alter table model_requests add column purchase_identity_id bigint;
create function snapshot_request_purchase_identity() returns trigger language plpgsql as $$
begin
    if TG_OP = 'INSERT' or OLD.provider_account_ref is distinct from NEW.provider_account_ref
        or NEW.attempt_count > OLD.attempt_count then
        NEW.purchase_identity_id := (select identity_id from account_purchase_bindings
            where provider_account_ref = NEW.provider_account_ref);
    else
        NEW.purchase_identity_id := OLD.purchase_identity_id;
    end if;
    return NEW;
end
$$;
-- Old logs have no principal snapshot. Before the latest account update a
-- rotated account may have belonged to a different workspace; do not guess.
update model_requests r set purchase_identity_id = b.identity_id
from account_purchase_bindings b join provider_accounts a on a.id = b.provider_account_ref
where r.provider_account_ref = b.provider_account_ref
  and (a.credential_revision = 1 or r.started_at >= a.updated_at);
create trigger model_requests_purchase_identity before insert or update of provider_account_ref, attempt_count
on model_requests for each row execute function snapshot_request_purchase_identity();

-- Existing cumulative entries already provide durable, transactional request-id deduplication.
insert into account_purchase_daily_usage (identity_id, usage_date, amount_usd)
select r.purchase_identity_id, (r.started_at at time zone 'Asia/Shanghai')::date, sum(e.amount)
from account_cumulative_cost_entries e join model_requests r on r.id = e.request_id
where e.currency = 'USD' and r.purchase_identity_id is not null
group by r.purchase_identity_id, (r.started_at at time zone 'Asia/Shanghai')::date;

update account_purchase_identities p set history_complete_from = now()
where exists (
    select 1 from account_purchase_bindings b
    join account_cumulative_cost_entries e on e.provider_account_ref = b.provider_account_ref
    left join model_requests r on r.id = e.request_id
    where b.identity_id = p.id and e.currency = 'USD'
      and (r.id is null or r.purchase_identity_id is null)
);

create function accumulate_account_purchase_usage() returns trigger language plpgsql as $$
declare
    request_identity bigint;
    request_day date;
begin
    if NEW.currency <> 'USD' then return null; end if;
    select purchase_identity_id, (started_at at time zone 'Asia/Shanghai')::date
    into request_identity, request_day from model_requests where id = NEW.request_id;
    if request_identity is null or request_day is null then return null; end if;
    insert into account_purchase_daily_usage (identity_id, usage_date, amount_usd)
    values (request_identity, request_day, NEW.amount)
    on conflict (identity_id, usage_date) do update
    set amount_usd = account_purchase_daily_usage.amount_usd + excluded.amount_usd;
    return null;
end
$$;
create trigger account_cumulative_entries_purchase_usage after insert on account_cumulative_cost_entries
for each row execute function accumulate_account_purchase_usage();

-- Add each offset to the original anchor, so January 31 does not drift to March 28.
create function account_purchase_period(anchor date, today date)
returns table (period_start date, period_end date) language sql immutable strict as $$
    with months as (
        select greatest(0, (extract(year from today)::int - extract(year from anchor)::int) * 12
            + extract(month from today)::int - extract(month from anchor)::int) as n
    ), offset_months as (
        select greatest(0, n - case when (anchor + make_interval(months => n))::date > today
            then 1 else 0 end) as n from months
    )
    select (anchor + make_interval(months => n))::date,
           (anchor + make_interval(months => n + 1))::date from offset_months
$$;
