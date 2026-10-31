-- Historical plans must not be inferred from the account's current subscription.
-- Existing rows remain unknown; new attempts capture the plan with identity.
alter table model_requests add column provider_account_plan_type_snapshot text;
alter table ops_events add column provider_account_plan_type_snapshot text;
