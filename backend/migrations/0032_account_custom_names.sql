ALTER TABLE provider_accounts
    ADD COLUMN custom_name text,
    ADD CONSTRAINT provider_accounts_custom_name_valid CHECK (
        custom_name IS NULL OR (
            char_length(custom_name) BETWEEN 1 AND 128
            AND custom_name = btrim(custom_name)
            AND custom_name !~ '[[:cntrl:]]'
        )
    );
