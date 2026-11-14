use std::{fs, os::unix::fs::PermissionsExt as _, path::PathBuf, time::Duration};

use gateway_admin::model::relogin::ReloginRequest;
use gateway_admin::ports::provider::ProviderAdminErrorKind;
use gateway_core::account::CredentialTarget;
use serde_json::{Value, json};

use crate::admin::{provider_ports, valid_config};

const WORKER_OUTPUT_ENV: &str = "CPR_TEST_RELOGIN_WORKER_OUTPUT";
const COMPLETED: &str = "relogin-error-contract-completed";
const COMMON_FAILURE: &str = "重登网络或上游验证失败，请检查代理或人工重新授权";

#[tokio::test]
async fn excel_failures_distinguish_interaction_loop_exchange_and_access() {
    let cases = [
        (
            "interaction_required",
            "mfa",
            "BPS 两步验证需要人工交互，请使用浏览器 OAuth 重新授权",
            "登录需要人工交互或流程已变化，请人工重新授权",
        ),
        (
            "login_loop",
            "authorize",
            "BPS 授权页面重复跳转，自动登录未完成，请使用浏览器 OAuth 重新授权",
            "登录需要人工交互或流程已变化，请人工重新授权",
        ),
        (
            "upstream_rejected",
            "token_exchange",
            "BPS 授权码交换令牌失败，请检查账号代理并重新授权",
            COMMON_FAILURE,
        ),
        (
            "transport_failed",
            "access_check",
            "BPS 凭据已取得，但权限检查请求失败，未保存或推送",
            COMMON_FAILURE,
        ),
        (
            "verification_failed",
            "access_check",
            "BPS 凭据已取得，但权限校验未通过，未保存或推送",
            "新凭据未通过验证，未推送",
        ),
    ]
    .into_iter()
    .map(|(code, stage, excel, codex)| {
        (
            json!({"ok": false, "code": code, "stage": stage}),
            [excel, codex],
        )
    })
    .collect();
    assert_worker_failures(
        "credential::relogin::excel_failures_distinguish_interaction_loop_exchange_and_access",
        cases,
    )
    .await;
}

#[tokio::test]
async fn excel_failure_messages_never_echo_worker_details_or_override_common_errors() {
    let mut cases = vec![
        (
            json!({
                "code": "interaction_required", "stage": "private-upstream-detail",
                "message": "private-upstream-detail", "url": "private-upstream-detail",
            }),
            [
                "BPS 授权页面需要人工交互，请使用浏览器 OAuth 重新授权",
                "登录需要人工交互或流程已变化，请人工重新授权",
            ],
        ),
        (
            json!({"code": "transport_failed", "stage": "private-upstream-detail"}),
            [COMMON_FAILURE; 2],
        ),
        (
            json!({"code": "workspace_ambiguous", "workspaces": [
                {"id": "fixture-a", "name": "A", "planType": "business"},
                {"id": "fixture-b", "name": "B", "planType": "business"},
            ]}),
            ["有多个同级工作区，请选择后继续"; 2],
        ),
    ];
    for (code, message) in [
        ("password_rejected", "账号密码被拒绝，请检查资料或账号状态"),
        ("mfa_rejected", "2FA 验证失败，请检查密钥和服务器时间"),
        ("account_banned", "账号已封禁，自动重登已停止，请人工处理"),
        ("unknown", COMMON_FAILURE),
    ] {
        cases.push((json!({"code": code}), [message; 2]));
    }
    assert_worker_failures(
        "credential::relogin::excel_failure_messages_never_echo_worker_details_or_override_common_errors",
        cases,
    )
    .await;
}

async fn assert_worker_failures(test_name: &str, cases: Vec<(Value, [&str; 2])>) {
    let Some(worker_output) = std::env::var_os(WORKER_OUTPUT_ENV) else {
        let directory = tempfile::tempdir().expect("isolated worker directory");
        let executable = directory.path().join("python3");
        fs::write(
            &executable,
            concat!(
                "#!/bin/sh\n",
                "[ \"$1\" = -I ] && [ \"$2\" = -c ] || exit 1\n",
                "/bin/cat >/dev/null\n",
                "/bin/cat \"$CPR_TEST_RELOGIN_WORKER_OUTPUT\"\n",
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let inherited_path = std::env::var_os("PATH").unwrap_or_default();
        let paths = std::iter::once(directory.path().to_path_buf())
            .chain(std::env::split_paths(&inherited_path));
        // Only the isolated test runner sees the fake worker; parallel tests keep their PATH.
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", test_name, "--nocapture"])
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env(
                WORKER_OUTPUT_ENV,
                directory.path().join("worker-output.json"),
            )
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(30), command.output())
            .await
            .expect("isolated relogin test timed out")
            .expect("run isolated relogin test");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            stdout.lines().filter(|line| *line == COMPLETED).count(),
            1,
            "isolated relogin assertions did not complete: {stdout}"
        );
        return;
    };
    let worker_output = PathBuf::from(worker_output);
    let config = valid_config();
    let bundle = provider_openai::initialize(config.config.clone(), provider_ports())
        .await
        .unwrap();
    for (payload, messages) in cases {
        fs::write(&worker_output, serde_json::to_vec(&payload).unwrap()).unwrap();
        for (credential_target, message) in [CredentialTarget::Excel, CredentialTarget::Codex]
            .into_iter()
            .zip(messages)
        {
            let error = bundle
                .admin_provider()
                .relogin(ReloginRequest {
                    credential_target,
                    email: "test@example.invalid".into(),
                    password: "fixture-password".into(),
                    mfa_secret: "fixture-mfa".into(),
                    workspace_id: None,
                    outbound_proxy: None,
                })
                .await
                .err()
                .expect("fixture worker failure");
            assert_eq!(error.kind(), ProviderAdminErrorKind::Unavailable);
            assert_eq!(error.public_message(), Some(message));
        }
    }
    println!("\n{COMPLETED}");
}
