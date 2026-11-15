alter table account_group_alert_policies
  add column bark_schedule_json jsonb not null
    default '{"timezone":"Asia/Shanghai","defaultEnabled":true,"rules":[]}'::jsonb
    check (jsonb_typeof(bark_schedule_json) = 'object' and octet_length(bark_schedule_json::text) <= 65536);

-- Escalations are created later but belong to the original confirmed condition.
alter table notification_outbox
  add column incident_activated_at timestamptz,
  add column bark_rule_key text;
update notification_outbox set incident_activated_at=created_at where not test;

alter table notification_outbox drop constraint notification_outbox_status_check;
alter table notification_outbox add constraint notification_outbox_status_check
  check (status in ('pending','sending','sent','failed','cancelled'));

create index notification_outbox_incident_idx
  on notification_outbox (group_id,channel,condition_kind,incident_activated_at)
  where not test;
