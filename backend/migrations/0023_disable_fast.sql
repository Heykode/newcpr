-- Fast 限制默认关闭；旧客户端省略更新字段时保留已有策略。
alter table runtime_settings add column disable_fast boolean not null default false;
alter table account_groups add column disable_fast boolean not null default false;
