alter table model_requests
    add column billing_snapshot_json jsonb
    check (billing_snapshot_json is null or jsonb_typeof(billing_snapshot_json) = 'object');
