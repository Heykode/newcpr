-- Align optional local names with upstream's 512-byte UTF-8 boundary.
-- All names accepted by the former 128-character limit remain valid.
alter table provider_accounts drop constraint provider_accounts_custom_name_valid;
alter table provider_accounts add constraint provider_accounts_custom_name_valid check (
  custom_name is null or (
    octet_length(custom_name) between 1 and 512
    and custom_name = btrim(custom_name)
    and custom_name !~ '[[:cntrl:]]'
  )
);
