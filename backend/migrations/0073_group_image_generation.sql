-- Existing groups keep their current image access. Existing runtime values remain unchanged.
alter table account_groups add column allow_image_generation boolean not null default true;
alter table runtime_settings alter column max_concurrent_per_account set default 5;
update runtime_settings set max_concurrent_per_account = 5
where current_setting('cpr.fresh_install', true) = 'true';
