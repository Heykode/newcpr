use crate::{model::AdminError, use_case::token_guard::DefaultTokenGuardService};
use futures::future::BoxFuture;
use gateway_core::{
    lifecycle::CancellationToken,
    task::{
        DaemonRestartPolicy, DaemonTask, WorkerContribution, WorkerId, WorkerKind,
        WorkerRegistration, WorkerRunnable, WorkerTaskError,
    },
};
use std::{sync::Arc, time::Duration};

pub(crate) fn contribution(
    service: Arc<DefaultTokenGuardService>,
) -> Result<WorkerContribution, AdminError> {
    let id = WorkerId::try_new(WorkerKind::OAuthRefresh, "admin_token_guard")
        .map_err(|_| AdminError::internal("token guard worker ID is invalid"))?;
    let restart = DaemonRestartPolicy::try_new(Duration::from_secs(5), Duration::from_secs(30))
        .map_err(|_| AdminError::internal("token guard restart policy is invalid"))?;
    let registration = WorkerRegistration::try_new(
        id,
        WorkerRunnable::Daemon {
            restart,
            task: Box::new(TokenGuardWorker(service)),
        },
    )
    .map_err(|_| AdminError::internal("token guard worker registration is invalid"))?;
    Ok(WorkerContribution::Registration(registration))
}

struct TokenGuardWorker(Arc<DefaultTokenGuardService>);
impl DaemonTask for TokenGuardWorker {
    fn run(&self, cancellation: CancellationToken) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(2));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    () = cancellation.cancelled() => return Ok(()),
                    _ = interval.tick() => {}
                }
                if self.0.tick(&cancellation).await.is_err() {
                    tracing::warn!("token guard cycle failed");
                }
            }
        })
    }
}
