-- Preserve request-time subscription descriptions without backfilling from current accounts.
alter table model_requests add column provider_account_plan_type_display_snapshot text;
alter table ops_events add column provider_account_plan_type_display_snapshot text;
-- PostgreSQL freezes star expansion at view creation. Append the new column after all existing columns.
do $$
declare old_columns text;
begin
  select string_agg(format('old.%I', attname), ', ' order by attnum) into old_columns
    from pg_attribute where attrelid = 'request_analytics_source'::regclass and attnum > 0 and not attisdropped;
  execute format('create or replace view request_analytics_source as select %s, mr.provider_account_plan_type_display_snapshot from (%s) old join model_requests mr on mr.id = old.id', old_columns, rtrim(pg_get_viewdef('request_analytics_source'::regclass, true), E'; \n'));
end $$;
