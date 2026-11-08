-- Relogin attempts are separate from pool-owned successful replacement counts.
CREATE TABLE account_relogin_history (
    entry_id text NOT NULL REFERENCES account_relogin_entries(id) ON DELETE CASCADE,
    id text NOT NULL,
    detail jsonb NOT NULL,
    PRIMARY KEY (entry_id, id)
);
