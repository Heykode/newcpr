ALTER TABLE account_relogin_settings
    ADD COLUMN max_retries integer NOT NULL DEFAULT 2 CHECK (max_retries BETWEEN 0 AND 10),
    ADD COLUMN retry_interval_minutes integer NOT NULL DEFAULT 5 CHECK (retry_interval_minutes BETWEEN 1 AND 1440);
