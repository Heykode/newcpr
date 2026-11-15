-- Preserve saved cleanup choices; never delete history during schema migration.
ALTER TABLE runtime_settings DROP CONSTRAINT runtime_settings_retention_ck;
ALTER TABLE runtime_settings ADD CONSTRAINT runtime_settings_retention_ck CHECK (
    usage_retention_days > 0
    AND ops_event_retention_days > 0
    AND audit_retention_days > 0
);

-- Short positive cleanup windows used to live outside the legacy statistics window.
-- Zero remains the explicit clear-all choice and must not become a statistics window.
UPDATE runtime_settings AS settings
SET usage_retention_days = (control.config #>> '{requests,retentionDays}')::bigint,
    ops_event_retention_days = (control.config #>> '{requests,retentionDays}')::bigint,
    config_revision = config_revision + 1
FROM log_cleanup_control AS control
WHERE settings.id = 1 AND control.singleton
  AND (control.config #>> '{requests,retentionDays}')::bigint BETWEEN 1 AND 30;
