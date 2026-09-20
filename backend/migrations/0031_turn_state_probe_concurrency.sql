ALTER TABLE runtime_settings
    ADD COLUMN turn_state_probe_concurrency INTEGER NOT NULL DEFAULT 3
    CHECK (turn_state_probe_concurrency BETWEEN 1 AND 10);
