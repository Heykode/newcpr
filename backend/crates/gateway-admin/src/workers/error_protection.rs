use crate::{model::AdminError, use_case::error_protection::ErrorProtectionRecoveryService};
use futures::future::BoxFuture;
use gateway_core::task::{
    DaemonRestartPolicy, DaemonTask, WorkerContribution, WorkerId, WorkerKind, WorkerRegistration,
    WorkerRunnable, WorkerTaskError,
};
use std::{sync::Arc, time::Duration};

pub(crate) fn contribution(
    service: Arc<ErrorProtectionRecoveryService>,
) -> Result<WorkerContribution, AdminError> {
    let delay = Duration::from_secs(30);
    let id = WorkerId::try_new(
        WorkerKind::RuntimeSnapshotReconciliation,
        "admin_error_protection",
    )
    .map_err(|_| AdminError::internal("invalid Error protection worker"))?;
    let restart = DaemonRestartPolicy::try_new(delay, delay)
        .map_err(|_| AdminError::internal("invalid recovery restart"))?;
    let registration = WorkerRegistration::try_new(
        id,
        WorkerRunnable::Daemon {
            restart,
            task: Box::new(RecoveryWorker(service)),
        },
    )
    .map_err(|_| AdminError::internal("invalid recovery registration"))?;
    Ok(WorkerContribution::Registration(registration))
}

struct RecoveryWorker(Arc<ErrorProtectionRecoveryService>);
impl DaemonTask for RecoveryWorker {
    fn run(
        &self,
        context: gateway_core::task::WorkerDaemonContext,
    ) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            let cancellation = context.cancellation().clone();
            let lane = || async {
                let mut interval = tokio::time::interval(Duration::from_secs(30));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tokio::select! { biased; ()=cancellation.cancelled()=>break, _=interval.tick()=>{} }
                    // Claims are acquired per probe, so a queued batch cannot outlive its leases.
                    for _ in 0..32 {
                        if cancellation.is_cancelled() {
                            break;
                        }
                        match self.0.run_one(cancellation.clone()).await {
                            Ok(true) => context.succeeded(),
                            Ok(false) => {
                                context.succeeded();
                                break;
                            }
                            Err(_) => {
                                tracing::warn!(
                                    "Error protection worker failed; scheduling was not restored"
                                );
                                break;
                            }
                        }
                    }
                }
            };
            lane().await;
            Ok(())
        })
    }
}
