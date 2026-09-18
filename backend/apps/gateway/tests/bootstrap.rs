use std::{fs, process::Command, sync::Arc, time::Duration};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use codex_proxy_rs::bootstrap::GatewayConfig;
use gateway_admin::{
    model::{
        MutationActor, MutationContext,
        provider_credentials::{
            AuthorizationMutationTarget, AuthorizationOwnerBinding, CompleteAuthorization,
            CredentialImportCommit, CredentialRotationCommit, PendingAuthorizationMutation,
            PrepareCredentialImport, PrepareCredentialRotation, ProviderDocument,
        },
    },
    ports::{provider::ProviderAdminErrorKind, store::AccountStore},
};
use gateway_core::{
    account::{OpaqueProviderData, ProviderAccountId, ProviderAccountStore},
    provider_ports::ProviderStorePorts,
    routing::ProviderKind,
};
use gateway_host::LoadableConfig;
use gateway_store::{
    postgres::{
        ObservabilityQueryBudget, PgAdminAccountStore, PgProviderAccountRepository,
        PgProviderEgressRepository, PgRuntimeSettingsRepository,
    },
    redis::{
        RedisCredentialCooldownRepository, RedisCredentialLeaseRepository,
        RedisCredentialStateRepository, RedisOAuthPendingFlowRepository,
        RedisProviderArtifactProfileRepository, RedisProviderLeaseCoordinator,
        RedisProviderSessionAffinityRepository, RedisProviderSessionExclusionRepository,
    },
};
use provider_openai::{OpenAiConfig, ProviderBundle, credential::CodexCredentialCodec};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

const CONFIG_EXAMPLE: &str = include_str!("../../../../deploy/config.example.yaml");
const POSTGRES_PASSWORD: &str = "111111111111111111111111111111111111111111111111";
const REDIS_PASSWORD: &str = "222222222222222222222222222222222222222222222222";
const ADMIN_PASSWORD: &str = "test-admin-password";
const TOPOLOGY_CHILD_ENV: &str = "CPR_TEST_TOPOLOGY_CHILD";

#[tokio::test]
async fn proxy_probe_should_use_provider_custom_ca_for_https_proxies() {
    use gateway_admin::ports::proxy::ProxyProbe;
    use gateway_core::account::OutboundProxy;
    use gateway_host::proxy_probe::HttpProxyProbe;
    use std::{sync::Arc, time::Duration};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio_rustls::{
        TlsAcceptor,
        rustls::{
            ServerConfig,
            pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject as _},
        },
    };

    const CHILD_ENV: &str = "CPR_TEST_PROXY_TLS_DIRECTORY";
    let Ok(directory) = std::env::var(CHILD_ENV) else {
        let directory = tempfile::tempdir().unwrap();
        generate_proxy_test_certificates(directory.path());
        // 使用子进程隔离环境变量，避免并行测试读取到临时 CA 配置。
        for ca_env in ["CODEX_CA_CERTIFICATE", "SSL_CERT_FILE"] {
            let mut child = Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "bootstrap::proxy_probe_should_use_provider_custom_ca_for_https_proxies",
                    "--nocapture",
                ])
                .env(CHILD_ENV, directory.path())
                .env_remove("CODEX_CA_CERTIFICATE")
                .env_remove("SSL_CERT_FILE")
                .env(ca_env, directory.path().join("ca.pem"));
            if ca_env == "CODEX_CA_CERTIFICATE" {
                child.env(
                    "SSL_CERT_FILE",
                    directory.path().join("missing-fallback.pem"),
                );
            }
            let output = child.output().unwrap();
            assert!(
                output.status.success(),
                "{ca_env}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        return;
    };
    provider_openai::ensure_rustls_provider();
    let directory = std::path::Path::new(&directory);
    let certificate = CertificateDer::from_pem_file(directory.join("server.pem")).unwrap();
    let key = PrivateKeyDer::from_pem_file(directory.join("server.key")).unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(
        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![certificate], key)
            .unwrap(),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy =
        OutboundProxy::parse(&format!("https://{}", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut stream = acceptor.accept(socket).await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            assert!(request.len() < 8192);
            request.push(stream.read_u8().await.unwrap());
        }
        assert!(request.starts_with(b"GET http://unresolvable.invalid/ip "));
        let body = "{\"ip\":\"203.0.113.8\"}";
        stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        stream.shutdown().await.unwrap();
    });
    let probe = HttpProxyProbe::new("http://unresolvable.invalid/ip")
        .with_client_builder(provider_openai::build_reqwest_client_with_custom_ca);
    let result = probe.test(&proxy).await;
    assert!(result.success, "{}", result.message);
    assert_eq!(result.exit_ip.unwrap().to_string(), "203.0.113.8");
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

fn generate_proxy_test_certificates(directory: &std::path::Path) {
    let openssl = |args: &[&str]| {
        let output = Command::new("openssl")
            .args(args)
            .current_dir(directory)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    openssl(&[
        "req",
        "-x509",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-keyout",
        "ca.key",
        "-out",
        "ca.pem",
        "-days",
        "1",
        "-subj",
        "/CN=Proxy Test CA",
    ]);
    openssl(&[
        "req",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-keyout",
        "server.key",
        "-out",
        "server.csr",
        "-subj",
        "/CN=localhost",
    ]);
    fs::write(directory.join("extensions"), "subjectAltName=IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n").unwrap();
    openssl(&[
        "x509",
        "-req",
        "-in",
        "server.csr",
        "-CA",
        "ca.pem",
        "-CAkey",
        "ca.key",
        "-CAcreateserial",
        "-out",
        "server.pem",
        "-days",
        "1",
        "-extfile",
        "extensions",
    ]);
}

#[test]
fn config_loader_should_load_complete_terminal_example() {
    parse_config(&valid_config()).expect("terminal config example");
}

#[test]
fn config_loader_should_resolve_paths_relative_to_config_file() {
    let (config, _directory) = parse_config(&valid_config()).expect("resolved config");
    let debug = format!("{config:?}");
    assert!(debug.contains(".runtime/data"));
    assert!(debug.contains(".runtime/logs"));
    assert!(debug.contains("frontend/dist"));
}

#[test]
fn config_loader_should_reject_missing_runtime_data_dir() {
    let config = valid_config().replace("  runtime_data_dir: '../.runtime/data'\n", "");

    assert!(parse_config(&config).is_err());
}

#[test]
fn config_loader_should_inject_connection_passwords_into_urls() {
    parse_config(&valid_config()).expect("Store validates password injection into both URLs");
}

#[test]
fn config_loader_should_apply_only_explicit_topology_overrides() {
    let invalid = valid_config()
        .replace("host: '127.0.0.1'", "host: ''")
        .replace("port: 8080", "port: 0")
        .replace(
            "url: 'postgres://codex_proxy@127.0.0.1:5432/codex_proxy'",
            "url: 'invalid-postgres-url'",
        )
        .replace("url: 'redis://127.0.0.1:6379/'", "url: 'invalid-redis-url'")
        .replace(POSTGRES_PASSWORD, "invalid-postgres-password")
        .replace(REDIS_PASSWORD, "invalid-redis-password");
    if std::env::var_os(TOPOLOGY_CHILD_ENV).is_some() {
        parse_config(&invalid).expect("explicit package-owned environment overrides");
        return;
    }
    assert!(parse_config(&invalid).is_err());
    let status = Command::new(std::env::current_exe().expect("current test executable"))
        .args([
            "--exact",
            "bootstrap::config_loader_should_apply_only_explicit_topology_overrides",
        ])
        .env(TOPOLOGY_CHILD_ENV, "1")
        .env("CPR_SERVER_HOST", "127.0.0.1")
        .env("CPR_SERVER_PORT", "8080")
        .env(
            "CPR_DATABASE_URL",
            "postgres://codex_proxy@127.0.0.1:5432/codex_proxy",
        )
        .env("CPR_REDIS_URL", "redis://127.0.0.1:6379/")
        .env("CPR_DATABASE_PASSWORD", POSTGRES_PASSWORD)
        .env("CPR_REDIS_PASSWORD", REDIS_PASSWORD)
        .status()
        .expect("run isolated environment override test");
    assert!(status.success());
}

#[test]
fn bootstrap_config_debug_should_redact_all_passwords() {
    let (config, _directory) = parse_config(&valid_config()).expect("config");
    let debug = format!("{config:?}");
    assert!(!debug.contains(POSTGRES_PASSWORD));
    assert!(!debug.contains(REDIS_PASSWORD));
    assert!(!debug.contains(ADMIN_PASSWORD));
    assert!(debug.contains("[REDACTED]"));
}

#[test]
fn startup_diagnostics_ignore_only_retired_fields_and_never_echo_values() {
    const CHILD_ENV: &str = "CPR_TEST_STARTUP_DIAGNOSTICS";
    const SECRET: &str = "retired-sensitive-value-must-not-be-printed";
    if std::env::var_os(CHILD_ENV).is_some() {
        gateway_host::load_config::<GatewayConfig>().expect("startup configuration");
        return;
    }
    for case in [
        "normal",
        "retired",
        "missing",
        "typo",
        "invalid",
        "wrong-type",
        "known-location",
    ] {
        let mut document: Value = config::Config::builder()
            .add_source(config::File::from_str(
                &valid_config(),
                config::FileFormat::Yaml,
            ))
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap();
        match case {
            "retired" => {
                document["openai"]["tls"] = json!({"secret": SECRET});
                document["openai"]["fingerprint"] = json!({"secret": SECRET});
                document["host"]["logging"]["file"]["max_files"] = json!(SECRET);
            }
            "missing" => {
                document["host"]["listen"]
                    .as_object_mut()
                    .unwrap()
                    .remove("port");
            }
            "typo" => {
                document["openai"]["wire_profile"]["codex_versoin"] = json!(SECRET);
            }
            "invalid" => {
                document["openai"]["tls"] = json!(SECRET);
                document["host"]["listen"]["port"] = json!(0);
            }
            "wrong-type" => {
                document["host"]["listen"]["port"] = json!(SECRET);
            }
            "known-location" => {
                document["openai"]["wire_profile"]["location"] = json!(SECRET);
            }
            _ => {}
        }
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("deploy")).unwrap();
        fs::write(
            directory.path().join("deploy/config.yaml"),
            document.to_string(),
        )
        .unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "bootstrap::startup_diagnostics_ignore_only_retired_fields_and_never_echo_values",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .current_dir(directory.path())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.success(),
            matches!(case, "normal" | "retired"),
            "{case}: {stderr}"
        );
        assert!(!stderr.contains(SECRET), "{case}: secret leaked");
        match case {
            "retired" => {
                for field in [
                    "openai.tls",
                    "openai.fingerprint",
                    "host.logging.file.max_files",
                ] {
                    assert!(stderr.contains(field), "{stderr}");
                }
            }
            "missing" => assert!(stderr.contains("host.listen.port"), "{stderr}"),
            "normal" => assert!(!stderr.contains("[warning]"), "{stderr}"),
            _ => {}
        }
    }
}

#[test]
fn config_loader_should_reject_unknown_fields() {
    assert_rejected(format!(
        "{}\nunknown_terminal_field: true\n",
        valid_config()
    ));
}

#[test]
fn config_loader_should_reject_removed_tls_section() {
    assert_rejected(valid_config().replace("openai:\n", "openai:\n  tls: {}\n"));
}

#[test]
fn config_loader_should_reject_missing_explicit_fields() {
    assert_rejected(valid_config().replace("  request_id_header: 'x-request-id'\n", ""));
}

#[test]
fn config_loader_should_reject_unsupported_schema_version() {
    assert_rejected(valid_config().replace("schema_version: 1", "schema_version: 2"));
}

#[test]
fn config_loader_should_reject_embedded_database_password() {
    assert_rejected(valid_config().replace(
        "postgres://codex_proxy@127.0.0.1:5432/codex_proxy",
        "postgres://codex_proxy:embedded@127.0.0.1:5432/codex_proxy",
    ));
}

#[test]
fn config_loader_should_reject_non_hex_postgres_password() {
    assert_rejected(valid_config().replace(POSTGRES_PASSWORD, &"g".repeat(48)));
}

#[test]
fn config_loader_should_reject_wrong_length_redis_password() {
    assert_rejected(valid_config().replace(REDIS_PASSWORD, "1234"));
}

#[test]
fn config_loader_should_reject_weak_admin_password() {
    assert_rejected(valid_config().replace(ADMIN_PASSWORD, "password"));
}

#[test]
fn config_loader_should_reject_admin_password_with_compose_interpolation() {
    assert_rejected(valid_config().replace(ADMIN_PASSWORD, "unsafe$password"));
}

#[test]
fn config_loader_should_reject_removed_fingerprint_section() {
    assert_rejected(valid_config().replace(
        "openai:\n",
        "openai:\n  fingerprint:\n    browser: removed\n",
    ));
}

#[test]
fn config_loader_should_reject_invalid_codex_cli_version() {
    assert_rejected(valid_config().replace("codex_version: '0.153.4'", "codex_version: 'latest'"));
}

#[test]
fn config_loader_should_reject_disabled_all_log_outputs() {
    assert_rejected(
        valid_config()
            .replace("stdout: true", "stdout: false")
            .replace("enabled: true", "enabled: false"),
    );
}

#[test]
fn config_loader_should_reject_zero_server_port() {
    assert_rejected(valid_config().replace("port: 8080", "port: 0"));
}

#[test]
fn config_loader_should_reject_invalid_desktop_profile_fields() {
    assert_rejected(valid_config().replace("desktop_build: '8109'", "desktop_build: 'build'"));
}

#[test]
fn config_loader_should_accept_default_and_custom_request_locations() {
    let original = valid_config();
    let omitted = original
        .lines()
        .filter(|line| !line.trim_start().starts_with("location:"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_ne!(original, omitted);
    parse_config(&omitted).expect("location defaults when omitted");
    let custom = original.replace(
        "location: { country: 'US', region: 'Ohio', city: 'Piketon', timezone: 'America/New_York' }",
        "location: { country: 'NZ', region: 'Auckland', city: 'Auckland', timezone: 'Pacific/Auckland' }",
    );
    assert_ne!(original, custom);
    parse_config(&custom).expect("custom location from YAML");
}

#[test]
fn config_loader_should_reject_invalid_request_location_timezones() {
    let original = valid_config();
    let invalid = original.replace("timezone: 'America/New_York'", "timezone: 'Not/A_Timezone'");
    assert_ne!(original, invalid);
    assert_rejected(invalid);
}

fn assert_rejected(config: String) {
    assert!(parse_config(&config).is_err());
}

fn valid_config() -> String {
    CONFIG_EXAMPLE
        .replacen(
            "password: &postgres_password ''",
            &format!("password: &postgres_password '{POSTGRES_PASSWORD}'"),
            1,
        )
        .replacen(
            "password: &redis_password ''",
            &format!("password: &redis_password '{REDIS_PASSWORD}'"),
            1,
        )
        .replace(
            "default_password: ''",
            &format!("default_password: '{ADMIN_PASSWORD}'"),
        )
}

fn parse_config(config: &str) -> Result<(GatewayConfig, tempfile::TempDir), String> {
    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
    let deploy = directory.path().join("deploy");
    fs::create_dir(&deploy).map_err(|error| error.to_string())?;
    let path = deploy.join("config.yaml");
    fs::write(&path, config).map_err(|error| error.to_string())?;
    let mut config = config::Config::builder()
        .add_source(config::File::from(path).required(true))
        .build()
        .and_then(config::Config::try_deserialize::<GatewayConfig>)
        .map_err(|error| error.to_string())?;
    config
        .resolve_and_validate(&deploy)
        .map_err(|error| error.to_string())?;
    Ok((config, directory))
}

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

struct Harness {
    admin_pool: PgPool,
    pool: PgPool,
    namespace: String,
    redis: redis::aio::ConnectionManager,
    repository: Arc<PgProviderAccountRepository>,
    store: PgAdminAccountStore,
    provider: ProviderBundle,
    server: MockServer,
    _runtime: tempfile::TempDir,
}

impl Harness {
    async fn create() -> Option<Self> {
        let database_url = test_endpoint("CPR_TEST_DATABASE_URL", "postgres")?;
        let redis_url = test_endpoint("CPR_TEST_REDIS_URL", "redis")
            .expect("Redis is required when the PostgreSQL integration test is enabled");
        let namespace = format!("cpr_principal_{}", Uuid::new_v4().simple());
        let admin_pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
            .expect("connect isolated PostgreSQL");
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "create schema \"{namespace}\""
        )))
        .execute(&admin_pool)
        .await
        .unwrap();
        let schema = namespace.clone();
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .after_connect(move |connection, _| {
                let schema = schema.clone();
                Box::pin(async move {
                    sqlx::query("select set_config('search_path', $1, false)")
                        .bind(schema)
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&database_url)
            .await
            .unwrap();
        MIGRATOR.run(&pool).await.unwrap();
        let redis = redis::Client::open(redis_url)
            .unwrap()
            .get_connection_manager()
            .await
            .expect("connect isolated Redis");
        let repository = Arc::new(PgProviderAccountRepository::new(pool.clone()));
        let state =
            Arc::new(RedisCredentialStateRepository::new(redis.clone(), &namespace).unwrap());
        let cooldowns =
            Arc::new(RedisCredentialCooldownRepository::new(redis.clone(), &namespace).unwrap());
        let (leases, _cleanup) = RedisProviderLeaseCoordinator::new(
            RedisCredentialLeaseRepository::new(redis.clone(), &namespace).unwrap(),
        );
        // Use the production adapters; no fake account store or token-derived Core command.
        let ports = ProviderStorePorts::new(
            repository.clone(),
            Arc::new(leases),
            Arc::new(
                RedisProviderSessionAffinityRepository::new(redis.clone(), &namespace).unwrap(),
            ),
            Arc::new(
                RedisProviderSessionExclusionRepository::new(redis.clone(), &namespace).unwrap(),
            ),
            state.clone(),
            Arc::new(
                RedisProviderArtifactProfileRepository::new(redis.clone(), &namespace).unwrap(),
            ),
            state,
            cooldowns.clone(),
            Arc::new(PgRuntimeSettingsRepository::new(pool.clone())),
            Arc::new(RedisOAuthPendingFlowRepository::new(redis.clone(), &namespace).unwrap()),
        )
        .with_egress(Arc::new(PgProviderEgressRepository::new(pool.clone())));
        let server = MockServer::start().await;
        let runtime = tempfile::tempdir().unwrap();
        let mut config = OpenAiConfig::default();
        config.api.base_url = server.uri();
        config.auth.oauth_token_endpoint = format!("{}/oauth/token", server.uri());
        config
            .resolve_and_validate(&runtime.path().join("deploy"))
            .unwrap();
        // Provider initialization registers the actual CodexDeviceCodec in this repository.
        let provider = provider_openai::initialize(config, ports).await.unwrap();
        let store = PgAdminAccountStore::with_repository(
            pool.clone(),
            repository.as_ref().clone(),
            Some(cooldowns),
            ObservabilityQueryBudget::try_new(1, Duration::from_secs(1)).unwrap(),
        );
        Some(Self {
            admin_pool,
            pool,
            namespace,
            redis,
            repository,
            store,
            provider,
            server,
            _runtime: runtime,
        })
    }

    async fn import(&self, access: &str, id: Option<&str>) -> ProviderAccountId {
        let prepared = self
            .provider
            .admin_provider()
            .prepare_import(PrepareCredentialImport {
                default_outbound_proxy: None,
                document: document(access, id),
            })
            .await
            .unwrap();
        self.store
            .commit_credential_import(
                CredentialImportCommit {
                    outbound_proxy: None,
                    settings: None,
                    prepared,
                },
                &context(),
            )
            .await
            .unwrap()
            .credential_ids
            .remove(0)
    }

    async fn rotate(
        &self,
        account_id: &ProviderAccountId,
        access: &str,
        id: Option<&str>,
    ) -> Result<(), gateway_admin::ports::provider::ProviderAdminError> {
        let account = self
            .store
            .credential_details(&ProviderKind::new("openai").unwrap(), account_id)
            .await
            .unwrap()
            .unwrap()
            .credential;
        let prepared = self
            .provider
            .admin_provider()
            .prepare_rotation(PrepareCredentialRotation {
                account,
                provider_material: document(access, id),
            })
            .await?;
        self.store
            .commit_credential_rotation(
                CredentialRotationCommit {
                    prepared: prepared.facts().clone(),
                    relogin_operation_id: None,
                },
                &context(),
            )
            .await
            .expect("commit actual Provider-prepared rotation");
        Ok(())
    }

    async fn snapshot(&self) -> Value {
        sqlx::query_scalar(
            "select jsonb_build_object(
                'accounts', (select jsonb_agg(to_jsonb(a) order by id) from provider_accounts a),
                'devices', (select jsonb_agg(to_jsonb(d) order by installation_id)
                            from provider_device_identities d),
                'settings', (select jsonb_agg(to_jsonb(s) order by id) from runtime_settings s),
                'audit', (select jsonb_agg(to_jsonb(e) order by id) from admin_audit_events e),
                'egress', (select jsonb_agg(to_jsonb(f) order by to_jsonb(f)::text)
                           from provider_egress_fixed_affinity f))",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    async fn close(mut self) {
        drop(self.provider);
        drop(self.store);
        drop(self.repository);
        let keys: Vec<String> = redis::cmd("KEYS")
            .arg(format!("{}:*", self.namespace))
            .query_async(&mut self.redis)
            .await
            .unwrap();
        if !keys.is_empty() {
            redis::cmd("DEL")
                .arg(keys)
                .query_async::<()>(&mut self.redis)
                .await
                .unwrap();
        }
        self.pool.close().await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "drop schema \"{}\" cascade",
            self.namespace
        )))
        .execute(&self.admin_pool)
        .await
        .unwrap();
        self.admin_pool.close().await;
    }
}

fn test_endpoint(name: &str, scheme: &str) -> Option<String> {
    let value = std::env::var(name).ok();
    assert!(
        value.is_some()
            || !std::env::var("CI")
                .is_ok_and(|value| !matches!(value.as_str(), "" | "0" | "false")),
        "{name} is required in CI"
    );
    let value = value?;
    let parsed = url::Url::parse(&value).expect("valid isolated test endpoint");
    assert_eq!(parsed.scheme(), scheme);
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    Some(value)
}

fn context() -> MutationContext {
    MutationContext {
        actor: MutationActor::AdminApiKey,
        request_id: format!("req_{}", Uuid::new_v4().simple()),
    }
}

fn token(user: &str, workspace: &str, email: &str, plan: &str) -> String {
    let payload = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&json!({
            "email": email,
            "https://api.openai.com/auth": {
                "chatgpt_user_id": user, "chatgpt_account_id": workspace,
                "chatgpt_plan_type": plan
            }
        }))
        .unwrap(),
    );
    format!("unverified-header.{payload}.unverified-signature")
}

fn document(access: &str, id: Option<&str>) -> ProviderDocument {
    ProviderDocument::new(OpaqueProviderData::new(
        json!({
            "access_token": access,
            "refresh_token": "synthetic-refresh",
            "id_token": id
        })
        .as_object()
        .unwrap()
        .clone(),
    ))
}

#[tokio::test]
async fn manual_rotation_commits_same_principal_and_rejects_conflict_or_unknown_without_pg_changes()
{
    let Some(harness) = Harness::create().await else {
        return;
    };
    let old_id = token("user-A", "workspace-A", "old@example.com", "pro");
    let id = harness.import("opaque-old", Some(&old_id)).await;
    let before = harness
        .repository
        .load_current_credential(&id)
        .await
        .unwrap();
    let old_device = CodexCredentialCodec::decode(&before.credential)
        .unwrap()
        .installation_id;
    let initial = harness.snapshot().await;
    let fresh_id = token("user-B", "workspace-A", "old@example.com", "plus");
    harness
        .rotate(&id, "opaque-new", Some(&fresh_id))
        .await
        .unwrap();
    let current = harness
        .repository
        .load_current_credential(&id)
        .await
        .unwrap();
    let data = CodexCredentialCodec::decode_complete(&current.credential).unwrap();
    let oauth = data.oauth().unwrap();
    assert_eq!(oauth.installation_id, old_device);
    assert_eq!(oauth.access_token, "opaque-new");
    assert_eq!(oauth.id_token.as_deref(), Some(fresh_id.as_str()));
    assert_eq!(current.account.email(), Some("old@example.com"));
    assert_eq!(current.account.plan_type(), Some("plus"));
    assert_eq!(current.account.upstream_user_id(), Some("user-B"));
    assert_eq!(current.account.upstream_account_id(), Some("workspace-A"));
    assert_eq!(
        current.account.revision().get(),
        before.account.revision().get() + 1
    );
    let committed = harness.snapshot().await;
    assert_eq!(committed["devices"].as_array().unwrap().len(), 1);
    assert_eq!(
        committed["devices"][0]["installation_id"],
        initial["devices"][0]["installation_id"]
    );
    assert_eq!(committed["devices"][0]["upstream_user_id"], "user-B");
    assert_eq!(
        committed["devices"][0]["upstream_account_id"],
        "workspace-A"
    );
    assert_eq!(
        committed["settings"][0]["config_revision"]
            .as_i64()
            .unwrap(),
        initial["settings"][0]["config_revision"].as_i64().unwrap() + 1
    );
    assert_eq!(committed["audit"].as_array().unwrap().len(), 2);

    let other_email = token("user-B", "workspace-A", "fresh@example.com", "plus");
    let other_workspace = token("user-B", "workspace-B", "old@example.com", "plus");
    for (access, new_id, message) in [
        (other_email.as_str(), None, "账号主体"),
        (other_workspace.as_str(), None, "账号主体"),
        (
            other_workspace.as_str(),
            Some(fresh_id.as_str()),
            "账号主体",
        ),
        (
            fresh_id.as_str(),
            Some(other_workspace.as_str()),
            "账号主体",
        ),
        ("opaque-unknown", None, "无法确认"),
    ] {
        let error = harness.rotate(&id, access, new_id).await.unwrap_err();
        assert_eq!(error.kind(), ProviderAdminErrorKind::Conflict);
        assert!(error.public_message().unwrap().contains(message));
        assert_eq!(harness.snapshot().await, committed);
    }
    let unresolved_id = harness.import("opaque-unresolved", None).await;
    let unresolved = harness.snapshot().await;
    let error = harness
        .rotate(&unresolved_id, "opaque-new", Some(&fresh_id))
        .await
        .unwrap_err();
    assert!(error.public_message().unwrap().contains("无法确认"));
    assert_eq!(harness.snapshot().await, unresolved);
    assert!(harness.server.received_requests().await.unwrap().is_empty());
    harness.close().await;
}

#[tokio::test]
async fn same_principal_reimport_uses_real_codec_and_keeps_the_persisted_device() {
    let Some(harness) = Harness::create().await else {
        return;
    };
    let old = token("user-A", "workspace-A", "old@example.com", "pro");
    let id = harness.import("opaque-old", Some(&old)).await;
    let before = harness
        .repository
        .load_current_credential(&id)
        .await
        .unwrap();
    let fresh = token("user-A", "workspace-A", "new@example.com", "plus");
    let imported_id = harness.import("opaque-reimported", Some(&fresh)).await;
    assert_eq!(imported_id, id);
    let after = harness
        .repository
        .load_current_credential(&id)
        .await
        .unwrap();
    let data = CodexCredentialCodec::decode_complete(&after.credential).unwrap();
    assert_eq!(
        data.oauth().unwrap().installation_id,
        CodexCredentialCodec::decode(&before.credential)
            .unwrap()
            .installation_id
    );
    assert_eq!(data.oauth().unwrap().access_token, "opaque-reimported");
    let snapshot = harness.snapshot().await;
    assert_eq!(snapshot["accounts"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["devices"].as_array().unwrap().len(), 1);
    assert_eq!(after.account.email(), Some("new@example.com"));
    harness.close().await;
}

#[tokio::test]
async fn loopback_reauthorization_rejects_wrong_or_unknown_principals_without_pg_changes() {
    let Some(harness) = Harness::create().await else {
        return;
    };
    let old = token("user-A", "workspace-A", "old@example.com", "pro");
    let id = harness.import("opaque-old", Some(&old)).await;
    let before = harness.snapshot().await;
    for (fresh, message) in [
        (
            token("user-B", "workspace-B", "other@example.com", "plus"),
            "账号主体",
        ),
        ("opaque-id".to_owned(), "无法确认"),
    ] {
        harness.server.reset().await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "opaque-new", "refresh_token": "synthetic-new-refresh",
                "id_token": fresh
            })))
            .expect(1)
            .mount(&harness.server)
            .await;
        let context = context();
        let started = harness
            .provider
            .admin_provider()
            .start_authorization(PendingAuthorizationMutation::new(
                ProviderKind::new("openai").unwrap(),
                AuthorizationMutationTarget::Reauthorize {
                    account_id: id.clone(),
                },
                AuthorizationOwnerBinding::from_context(&context),
            ))
            .await
            .unwrap();
        let inner = url::Url::parse(&started.authorization_url)
            .unwrap()
            .query_pairs()
            .find(|(key, _)| key == "authorize_url")
            .expect("Desktop wrapper contains the OAuth authorization URL")
            .1
            .into_owned();
        let state = url::Url::parse(&inner)
            .unwrap()
            .query_pairs()
            .find(|(key, _)| key == "state")
            .expect("inner OAuth authorization URL contains state")
            .1
            .into_owned();
        let error = harness
            .provider
            .admin_provider()
            .complete_authorization(CompleteAuthorization {
                settings: None,
                context,
                flow_id: started.flow_id,
                callback_url: format!(
                    "http://localhost:1455/auth/callback?code=synthetic&state={state}"
                ),
            })
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ProviderAdminErrorKind::Conflict);
        assert!(error.public_message().unwrap().contains(message));
        assert!(!error.public_message().unwrap().contains("无效"));
        assert_eq!(harness.snapshot().await, before);
        harness.server.verify().await;
    }
    harness.close().await;
}
