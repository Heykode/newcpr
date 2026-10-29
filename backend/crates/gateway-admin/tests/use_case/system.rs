use std::sync::Mutex;

use async_trait::async_trait;

use gateway_admin::{
    model::system::{
        SystemOperationAccepted, SystemOperationState, SystemOperationStatus, SystemUpdateDetail,
        SystemUpdateStatus, SystemVersion,
    },
    ports::system::{SystemOperationError, SystemOperations, SystemUpdateEventStream},
};

#[derive(Default)]
struct RecordingSystemOperations {
    target: Mutex<Option<Option<String>>>,
    failure: Option<SystemOperationError>,
}

#[async_trait]
impl SystemOperations for RecordingSystemOperations {
    async fn version(&self) -> Result<SystemVersion, SystemOperationError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        Ok(SystemVersion {
            version: "1.0.0".to_owned(),
            git_sha: "unknown".to_owned(),
            build_time: "unknown".to_owned(),
            deployment_mode: "source".to_owned(),
            update_channel: "stable".to_owned(),
            latest_version: "1.0.0".to_owned(),
            has_update: false,
            update_cached: false,
            update_warning: None,
        })
    }

    async fn update_detail(&self, _: bool) -> Result<SystemUpdateDetail, SystemOperationError> {
        Ok(SystemUpdateDetail {
            current_version: "1.0.0".to_owned(),
            latest_version: "1.0.0".to_owned(),
            has_update: false,
            deployment_mode: "source".to_owned(),
            build_type: "source".to_owned(),
            release_url: None,
            notes: None,
            cached: false,
            update_supported: false,
            unsupported_reason: Some("source build".to_owned()),
            warning: None,
        })
    }

    fn update_events(&self) -> SystemUpdateEventStream {
        Box::pin(futures::stream::empty())
    }

    async fn perform_update(
        &self,
        target_version: Option<String>,
    ) -> Result<SystemOperationAccepted, SystemOperationError> {
        *self.target.lock().expect("target") = Some(target_version.clone());
        Ok(SystemOperationAccepted::Update {
            operation_id: "operation-update".to_owned(),
            deployment_mode: "source".to_owned(),
            message: "accepted".to_owned(),
            target_version: target_version.unwrap_or_else(|| "latest".to_owned()),
        })
    }

    async fn update_status(&self) -> Result<SystemUpdateStatus, SystemOperationError> {
        Ok(SystemUpdateStatus {
            need_restart: false,
            previous_version: None,
            current_version: Some("1.0.0".to_owned()),
            operation: SystemOperationState {
                operation_id: None,
                kind: None,
                status: SystemOperationStatus::Idle,
                target_version: None,
                message: None,
                error: None,
                started_at: None,
                finished_at: None,
            },
        })
    }

    async fn rollback(&self) -> Result<SystemOperationAccepted, SystemOperationError> {
        Ok(SystemOperationAccepted::Rollback {
            operation_id: "operation-rollback".to_owned(),
            message: "accepted".to_owned(),
            need_restart: true,
        })
    }

    async fn restart(&self) -> Result<SystemOperationAccepted, SystemOperationError> {
        Ok(SystemOperationAccepted::Restart {
            operation_id: "operation-restart".to_owned(),
            message: "accepted".to_owned(),
        })
    }
}

#[tokio::test]
async fn system_failure_should_preserve_safe_message_source_and_redaction_marker() {
    let operations = std::sync::Arc::new(RecordingSystemOperations {
        failure: Some(
            SystemOperationError::new(
                gateway_admin::ports::system::SystemOperationErrorKind::Upstream,
                "更新包下载失败",
            )
            .with_source(std::io::Error::other("native transport diagnostic"))
            .redact_sensitive_context(),
        ),
        ..RecordingSystemOperations::default()
    });
    let services = super::AdminHarness::new().system(operations).build().await;
    let error = services.system().version().await.expect_err("host failure");
    assert_eq!(
        error.kind(),
        gateway_admin::model::AdminErrorKind::BadGateway
    );
    assert_eq!(error.message(), "更新包下载失败");
    assert!(!format!("{error} {error:?}").contains("native transport diagnostic"));
    let details = error.error_details().expect("host error details");
    assert!(details.as_str().contains("native transport diagnostic"));
    let details: serde_json::Value = serde_json::from_str(details.as_str()).expect("details JSON");
    assert_eq!(details["redacted"], true);
}

#[tokio::test]
async fn system_update_should_normalize_blank_target_to_latest() {
    let operations = std::sync::Arc::new(RecordingSystemOperations::default());
    let services = super::AdminHarness::new()
        .system(operations.clone())
        .build()
        .await;
    services
        .system()
        .perform_update(Some("   ".to_owned()))
        .await
        .expect("perform update");

    assert_eq!(*operations.target.lock().expect("target"), Some(None));
}
