-- 本地 OAuth 账号观测的公开授权，旧 Key 默认关闭。
alter table client_api_keys
  add column peer_sharing_json jsonb not null
  default '{"enabled":false,"sharePool":false,"shareQuota":false,"sharePlans":false}';
