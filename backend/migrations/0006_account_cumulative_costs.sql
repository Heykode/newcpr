-- Keep historical totals independent of expiring request logs and credential imports.
create table account_cumulative_costs (
    provider_account_ref text not null,
    currency text not null,
    amount numeric(40,10) not null check (amount >= 0),
    request_count bigint not null check (request_count >= 0),
    primary key (provider_account_ref, currency)
);

create table account_cumulative_cost_entries (
    request_id text primary key,
    provider_account_ref text not null,
    currency text not null,
    amount numeric(20,10) not null check (amount >= 0)
);

-- A known cost is itself usage evidence. Keep delivery rules aligned with
-- completed_usage_fact_predicate in gateway-store/src/postgres/usage_facts.rs.
create function account_cumulative_cost_is_countable(request model_requests)
returns boolean language sql immutable as $$
    select (request.provider_account_ref is not null
       and request.cost_amount is not null
       and request.cost_currency is not null
       and request.outcome = 'succeeded'
       and request.downstream_committed_at is not null
       and ((request.client_transport = 'websocket' and request.client_status_code is null)
            or request.client_status_code between 200 and 399)) is true
$$;

create function accumulate_account_cost()
returns trigger language plpgsql as $$
declare
    inserted_count bigint;
begin
    if TG_OP = 'UPDATE' and account_cumulative_cost_is_countable(OLD) then
        return null;
    end if;

    if not account_cumulative_cost_is_countable(NEW) then
        return null;
    end if;

    -- Completion is immutable in the repository. Retain the first known cost
    -- once per request, independently of log retention and repeated observations.
    insert into account_cumulative_cost_entries (
        request_id, provider_account_ref, currency, amount
    ) values (
        NEW.id, NEW.provider_account_ref, NEW.cost_currency, NEW.cost_amount
    ) on conflict (request_id) do nothing;
    get diagnostics inserted_count = row_count;

    if inserted_count = 1 then
        insert into account_cumulative_costs (
            provider_account_ref, currency, amount, request_count
        ) values (
            NEW.provider_account_ref, NEW.cost_currency, NEW.cost_amount, 1
        ) on conflict (provider_account_ref, currency) do update
        set amount = account_cumulative_costs.amount + excluded.amount,
            request_count = account_cumulative_costs.request_count + 1;
    end if;
    return null;
end
$$;

-- Backfill and trigger activation share the migration transaction. Writers cannot
-- enter between them; previously pruned history cannot be reconstructed.
lock table model_requests in share row exclusive mode;

insert into account_cumulative_costs (provider_account_ref, currency, amount, request_count)
select mr.provider_account_ref, mr.cost_currency, sum(mr.cost_amount), count(*)
from model_requests mr
where account_cumulative_cost_is_countable(mr)
group by mr.provider_account_ref, mr.cost_currency;

insert into account_cumulative_cost_entries (request_id, provider_account_ref, currency, amount)
select mr.id, mr.provider_account_ref, mr.cost_currency, mr.cost_amount
from model_requests mr
where account_cumulative_cost_is_countable(mr);

create trigger model_requests_accumulate_account_cost
after insert or update of outcome, downstream_committed_at, client_transport,
    client_status_code, cost_amount, cost_currency, provider_account_ref
on model_requests
for each row execute function accumulate_account_cost();
