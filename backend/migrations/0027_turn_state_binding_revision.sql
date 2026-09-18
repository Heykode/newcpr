-- Preserve only State rows already owned by the current credential.
alter table provider_accounts
  add column turn_state_binding_revision bigint not null default 1;

update provider_accounts
set turn_state_binding_revision = credential_revision;

alter table provider_accounts
  add constraint provider_accounts_turn_state_binding_revision_ck
  check (turn_state_binding_revision > 0
         and turn_state_binding_revision <= credential_revision);

comment on column provider_turn_states.credential_revision is
  'State binding revision, not the credential-material write CAS (since migration 0027).';
