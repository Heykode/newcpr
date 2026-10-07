alter table account_groups
    add column fast_mode text not null default 'default'
    check (fast_mode in ('default', 'enabled', 'disabled'));

-- Preserve the existing group opt-out when introducing the three-state control.
update account_groups set fast_mode = 'disabled' where disable_fast;
