ALTER TABLE account_relogin_settings
    DROP CONSTRAINT account_relogin_settings_retry_interval_minutes_check;

ALTER TABLE account_relogin_settings
    RENAME COLUMN retry_interval_minutes TO retry_interval_seconds;

UPDATE account_relogin_settings
SET retry_interval_seconds = CASE
    WHEN retry_interval_seconds = 5 THEN 30
    ELSE retry_interval_seconds * 60
END;

ALTER TABLE account_relogin_settings
    ALTER COLUMN retry_interval_seconds SET DEFAULT 30,
    ADD CONSTRAINT account_relogin_settings_retry_interval_seconds_check
        CHECK (retry_interval_seconds BETWEEN 1 AND 86400);
