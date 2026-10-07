use crate::{model::AdminError, use_case::reset_credits::ResetCreditsService};
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
    service: Arc<ResetCreditsService>,
) -> Result<WorkerContribution, AdminError> {
    let delay = Duration::from_secs(2);
    let id = WorkerId::try_new(
        WorkerKind::RuntimeSnapshotReconciliation,
        "admin_reset_credits",
    )
    .map_err(|_| AdminError::internal("invalid reset worker ID"))?;
    let restart = DaemonRestartPolicy::try_new(delay, delay)
        .map_err(|_| AdminError::internal("invalid reset worker restart"))?;
    Ok(WorkerContribution::Registration(
        WorkerRegistration::try_new(
            id,
            WorkerRunnable::Daemon {
                restart,
                task: Box::new(ResetWorker(service)),
            },
        )
        .map_err(|_| AdminError::internal("invalid reset worker registration"))?,
    ))
}
struct ResetWorker(Arc<ResetCreditsService>);
impl DaemonTask for ResetWorker {
    fn run(&self, cancellation: CancellationToken) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            let lane = || async {
                loop {
                    tokio::select! { biased; () = cancellation.cancelled() => break,
                    () = tokio::time::sleep(Duration::from_secs(1)) => {} }
                    if self.0.run_one().await.is_err() {
                        tracing::warn!(
                            "reset credit job could not update; no automatic consumption retry"
                        );
                    }
                }
            };
            let scanner = async {
                loop {
                    tokio::select! { biased; () = cancellation.cancelled() => break,
                    () = tokio::time::sleep(Duration::from_secs(1)) => {} }
                    // Bounded scan; durable per-account due times and leases coordinate instances.
                    for _ in 0..100 {
                        if cancellation.is_cancelled() {
                            break;
                        }
                        match self.0.check_auto_one().await {
                            Ok(true) => {}
                            Ok(false) => break,
                            Err(_) => {
                                tracing::warn!(
                                    "automatic reset check could not update; no consumption retry"
                                );
                                break;
                            }
                        }
                    }
                }
            };
            futures::future::join(futures::future::join_all((0..3).map(|_| lane())), scanner).await;
            Ok(())
        })
    }
}
