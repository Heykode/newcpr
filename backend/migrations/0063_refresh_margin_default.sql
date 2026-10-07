-- Align only the historical implicit default with the approved refresh window.
update runtime_settings
set refresh_margin_seconds = 300,
    config_revision = config_revision + 1,
    updated_at = now()
where id = 1 and refresh_margin_seconds = 3600;
