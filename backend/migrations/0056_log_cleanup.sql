-- No historical records are deleted by this migration.
CREATE TABLE log_cleanup_control (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    revision bigint NOT NULL DEFAULT 1 CHECK (revision > 0),
    config jsonb NOT NULL,
    next_run_at timestamptz,
    job jsonb
);
INSERT INTO log_cleanup_control(config)
SELECT jsonb_build_object(
    'enabled', false, 'frequency', 'daily', 'dailyHour', 3, 'dailyMinute', 0,
    'timezone', 'Asia/Shanghai',
    'requests', jsonb_build_object('selected', true, 'retentionDays', usage_retention_days),
    'files', jsonb_build_object('selected', true, 'retentionDays', 7),
    'captures', jsonb_build_object('selected', false, 'retentionDays', 7),
    'audit', jsonb_build_object('selected', false, 'retentionDays', audit_retention_days)
) FROM runtime_settings WHERE id = 1;
