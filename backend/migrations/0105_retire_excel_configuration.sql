-- Retire only Excel settings. Keep ordinary template IDs, revisions, references,
-- account credentials, scheduling, egress and historical usage records intact.
-- The new readers already discard these exact fields; this persists that same
-- canonical representation without treating it as a user template edit.
create function pg_temp.cpr_retire_excel_template(value jsonb) returns jsonb
language sql immutable as $$
    select case when jsonb_typeof(value) = 'object' then value - array[
        'responsesUpstream', 'excelModels', 'excelModelsFollowGlobal',
        'excelCacheCreationAsInput', 'excelIgnoreEncryptedContent',
        'excelAutoDisableOn403', 'excel403Action', 'excelRecovery'
    ] else value end
$$;

create function pg_temp.cpr_retire_excel_quality(value jsonb) returns jsonb
language sql immutable as $$
    select case when jsonb_typeof(clean #> '{failureTemplate,config}') = 'object'
        then jsonb_set(clean, '{failureTemplate,config}',
            pg_temp.cpr_retire_excel_template(clean #> '{failureTemplate,config}'))
        else clean end
    from (select (value - array['disableExcelOnNativeRecovery', 'excelRecoveryThreshold'])
        || case when value->>'failureAction' = 'enable_excel'
            then '{"failureAction":"none"}'::jsonb else '{}'::jsonb end as clean) normalized
$$;

update account_relogin_templates
set config = pg_temp.cpr_retire_excel_template(config)
where config is distinct from pg_temp.cpr_retire_excel_template(config);

update quality_rules
set config = pg_temp.cpr_retire_excel_quality(config),
    recovery = case when config->>'failureAction' = 'enable_excel'
        then recovery - array['excel_owner', 'excel_pass_streak', 'excel_streak']
        else recovery end
where config is distinct from pg_temp.cpr_retire_excel_quality(config);

update quality_rule_templates
set config = pg_temp.cpr_retire_excel_quality(config)
where config is distinct from pg_temp.cpr_retire_excel_quality(config);

update quality_group_rules
set config = pg_temp.cpr_retire_excel_quality(config)
where config is distinct from pg_temp.cpr_retire_excel_quality(config);

update account_relogin_entries
set material = jsonb_set(material, '{enrollment,config}',
    pg_temp.cpr_retire_excel_template(material #> '{enrollment,config}'))
where jsonb_typeof(material #> '{enrollment,config}') = 'object'
    and material #> '{enrollment,config}' is distinct from
        pg_temp.cpr_retire_excel_template(material #> '{enrollment,config}');

update account_relogin_entries
set material = jsonb_set(material, '{enrollment_plan,enrollment,config}',
    pg_temp.cpr_retire_excel_template(material #> '{enrollment_plan,enrollment,config}'))
where jsonb_typeof(material #> '{enrollment_plan,enrollment,config}') = 'object'
    and material #> '{enrollment_plan,enrollment,config}' is distinct from
        pg_temp.cpr_retire_excel_template(material #> '{enrollment_plan,enrollment,config}');

-- Dedicated workers have been removed; leave their historical tables in place.
-- Remove only their triggers so ordinary account/settings writes no longer do
-- recovery bookkeeping. No shared trigger or account row is altered.
drop trigger if exists invalidate_excel_recovery on provider_accounts;
drop trigger if exists invalidate_excel_recovery_runtime on runtime_settings;
drop function if exists invalidate_excel_recovery();
drop function if exists invalidate_excel_recovery_runtime();
update runtime_settings
set request_tuning_json = request_tuning_json - array[
    'excelImageRelayBytes', 'excelImageMaxBytes', 'excelImageTotalBytes',
    'excelImageMaxCount', 'excelImageLimitPolicy', 'excelImageWarningRemaining',
    'excelImageCompactReserve', 'excelImageRelayRequests', 'excelImageRelayDownloads',
    'excelImageRelayEntries', 'excelImageRelayTtlMinutes', 'excelImageTransport'
];
delete from account_excel_recovery;
update excel_recovery_slots set lease_id = null, lease_until = null;

drop function pg_temp.cpr_retire_excel_quality(jsonb);
drop function pg_temp.cpr_retire_excel_template(jsonb);
