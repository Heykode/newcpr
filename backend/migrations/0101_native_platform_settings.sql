-- 原生隐私与价目基础；旧设置默认保持关闭或空覆盖。
alter table runtime_settings add column codex_privacy_policy_json jsonb not null
    default '{"enabled":false,"onError":"skip_rule","rules":[]}'::jsonb
    check (jsonb_typeof(codex_privacy_policy_json) = 'object'
        and jsonb_typeof(codex_privacy_policy_json->'enabled') = 'boolean'
        and codex_privacy_policy_json->>'onError' in ('skip_rule', 'reject_request')
        and jsonb_typeof(codex_privacy_policy_json->'rules') = 'array'
        and codex_privacy_policy_json ?& array['enabled', 'onError', 'rules']);
alter table runtime_settings add column pricing_overrides_json jsonb not null default '{}'
    check (jsonb_typeof(pricing_overrides_json) = 'object');
alter table runtime_settings add column pricing_synced_json jsonb not null default '{}'
    check (jsonb_typeof(pricing_synced_json) = 'object');
alter table runtime_settings add column pricing_synced_at timestamptz;

-- 设置草稿仅因真实设置写入冲突，发布版本仍由全部运行配置共享。
alter table runtime_settings add column settings_revision bigint not null default 1;
update runtime_settings set settings_revision = config_revision;
alter table runtime_settings add constraint runtime_settings_edit_revision_ck
    check (settings_revision between 1 and config_revision);

-- 本地旧 TTL 列为 binding，不是官方的 affinity；重命名保留值。
alter table runtime_settings rename column openai_account_affinity to account_affinity;
alter table runtime_settings rename column openai_session_binding_ttl_hours to session_affinity_ttl_hours;
alter table runtime_settings rename constraint runtime_settings_openai_account_affinity_check to runtime_settings_account_affinity_check;
alter table runtime_settings rename constraint runtime_settings_openai_session_binding_ttl_hours_check to runtime_settings_session_affinity_ttl_hours_check;
