-- Preserve accepted request settlement and historical charges after key deletion.
alter table client_key_budget_windows
    drop constraint client_key_budget_windows_client_api_key_id_fkey;
alter table client_key_charge_events
    drop constraint client_key_charge_events_client_api_key_id_fkey;
