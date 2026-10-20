-- Restored global usage only. Live requests remain the source of online statistics.
CREATE TABLE backup_usage_hours (
    bucket_start timestamptz PRIMARY KEY,
    counts jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(counts) = 'object'),
    costs jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(costs) = 'object')
);
