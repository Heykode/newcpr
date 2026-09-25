CREATE TABLE request_capture_config (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    config jsonb NOT NULL
);
INSERT INTO request_capture_config (config)
VALUES ('{"enabled":false,"quotaMib":1024,"retentionDays":7}'::jsonb);

CREATE TABLE request_capture_tasks (
    id uuid PRIMARY KEY,
    instance_id uuid NOT NULL,
    task jsonb NOT NULL,
    expires_at timestamptz NOT NULL
);
CREATE INDEX request_capture_tasks_instance ON request_capture_tasks (instance_id, expires_at);

CREATE TABLE request_capture_records (
    id uuid PRIMARY KEY,
    task_id uuid NOT NULL REFERENCES request_capture_tasks(id) ON DELETE CASCADE,
    record jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX request_capture_records_task ON request_capture_records (task_id, created_at);
