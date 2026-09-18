-- Old opaque values must not survive an account credential replacement.
alter table provider_turn_states
  add column credential_revision bigint not null default 0,
  add constraint provider_turn_states_credential_revision_ck check (credential_revision >= 0);
