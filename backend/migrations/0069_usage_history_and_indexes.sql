-- Historical names are captured at admission. Legacy backfill can only use
-- names that still exist at upgrade time; deleted names remain unknown.
alter table model_requests add column client_api_key_name_snapshot text;

update model_requests request
set client_api_key_name_snapshot = client_key.name
from client_api_keys client_key
where client_key.id = request.client_api_key_ref;

-- Match NewCPR's completed_usage_fact_predicate, including quality isolation.
create index model_requests_delivered_client_idx
on model_requests (client_api_key_ref, started_at, id)
where outcome = 'succeeded' and downstream_committed_at is not null
  and (request_kind is distinct from 'account_quality_check'
       or protocol is distinct from 'admin_quality_check'
       or client_transport is distinct from 'internal')
  and (provider_kind is distinct from 'openai' or request_kind is distinct from 'prewarm')
  and ((client_transport = 'websocket' and client_status_code is null)
       or client_status_code between 200 and 399)
  and (requested_model_id is not null or upstream_model_id is not null
       or image_generation_requested or input_tokens is not null
       or output_tokens is not null or cached_tokens is not null
       or cache_write_tokens is not null or reasoning_tokens is not null
       or image_input_tokens is not null or image_output_tokens is not null
       or total_tokens is not null or cost_amount is not null);

create index model_requests_delivered_account_idx
on model_requests (provider_account_ref, started_at, id)
where outcome = 'succeeded' and downstream_committed_at is not null
  and (request_kind is distinct from 'account_quality_check'
       or protocol is distinct from 'admin_quality_check'
       or client_transport is distinct from 'internal')
  and (provider_kind is distinct from 'openai' or request_kind is distinct from 'prewarm')
  and ((client_transport = 'websocket' and client_status_code is null)
       or client_status_code between 200 and 399)
  and (requested_model_id is not null or upstream_model_id is not null
       or image_generation_requested or input_tokens is not null
       or output_tokens is not null or cached_tokens is not null
       or cache_write_tokens is not null or reasoning_tokens is not null
       or image_input_tokens is not null or image_output_tokens is not null
       or total_tokens is not null or cost_amount is not null);
