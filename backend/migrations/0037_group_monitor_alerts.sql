create table notification_channels (
  id bigint primary key check (id = 1),
  smtp_enabled boolean not null default false,
  smtp_host text not null default '',
  smtp_port bigint not null default 587 check (smtp_port between 1 and 65535),
  smtp_security text not null default 'starttls' check (smtp_security in ('none','starttls','tls')),
  smtp_username text,
  smtp_password text,
  smtp_from_name text,
  smtp_from_email text,
  bark_enabled boolean not null default false,
  bark_server_url text not null default 'https://api.day.app',
  bark_device_key text,
  bark_level text not null default 'active' check (bark_level in ('passive','active','timeSensitive','critical')),
  bark_sound text,
  bark_volume bigint not null default 5 check (bark_volume between 0 and 10),
  bark_call boolean not null default false,
  updated_at timestamptz not null
);

insert into notification_channels (id, updated_at) values (1, now());

create table account_group_alert_policies (
  group_id text primary key references account_groups(id) on delete cascade,
  enabled boolean not null default false,
  concurrency_enabled boolean not null default true,
  concurrency_threshold double precision not null default 90 check (concurrency_threshold > 0 and concurrency_threshold <= 100),
  concurrency_confirmation_seconds bigint not null default 20 check (concurrency_confirmation_seconds between 0 and 86400),
  eta_enabled boolean not null default true,
  eta_threshold_minutes double precision not null default 10 check (eta_threshold_minutes >= 0 and eta_threshold_minutes <= 525600),
  eta_confirmation_seconds bigint not null default 30 check (eta_confirmation_seconds between 0 and 86400),
  quota_zero_enabled boolean not null default true,
  quota_zero_confirmation_seconds bigint not null default 0 check (quota_zero_confirmation_seconds between 0 and 86400),
  availability_enabled boolean not null default true,
  availability_confirmation_seconds bigint not null default 0 check (availability_confirmation_seconds between 0 and 86400),
  email_enabled boolean not null default false,
  email_recipients_json jsonb not null default '[]'::jsonb check (jsonb_typeof(email_recipients_json) = 'array' and octet_length(email_recipients_json::text) <= 16384),
  bark_enabled boolean not null default false,
  bark_level text check (bark_level is null or bark_level in ('passive','active','timeSensitive','critical')),
  bark_sound text,
  bark_volume bigint check (bark_volume is null or bark_volume between 0 and 10),
  bark_call boolean,
  updated_at timestamptz not null
);

create table account_group_alert_incidents (
  group_id text not null references account_groups(id) on delete cascade,
  condition_kind text not null check (condition_kind in ('concurrency','eta','quota_zero','availability')),
  first_active_at timestamptz,
  activated_at timestamptz,
  recovery_started_at timestamptz,
  updated_at timestamptz not null,
  primary key (group_id, condition_kind)
);

create table notification_outbox (
  id text primary key,
  group_id text references account_groups(id) on delete set null,
  condition_kind text,
  channel text not null check (channel in ('email','bark')),
  target text not null,
  subject text not null,
  body text not null,
  bark_level text not null default 'active' check (bark_level in ('passive','active','timeSensitive','critical')),
  bark_sound text,
  bark_volume bigint not null default 5 check (bark_volume between 0 and 10),
  bark_call boolean not null default false,
  test boolean not null default false,
  status text not null default 'pending' check (status in ('pending','sending','sent','failed')),
  attempts bigint not null default 0 check (attempts >= 0),
  next_attempt_at timestamptz not null,
  claim_started_at timestamptz,
  error_summary text,
  created_at timestamptz not null,
  finished_at timestamptz
);

create index notification_outbox_due_idx
  on notification_outbox (next_attempt_at, created_at)
  where status in ('pending','sending');
