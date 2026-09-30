use crate::{model::AdminError, use_case::excel_recovery::ExcelRecoveryService};
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
    service: Arc<ExcelRecoveryService>,
) -> Result<WorkerContribution, AdminError> {
    let delay = Duration::from_secs(60);
    let id = WorkerId::try_new(
        WorkerKind::RuntimeSnapshotReconciliation,
        "admin_excel_recovery",
    )
    .map_err(|_| AdminError::internal("invalid Excel recovery worker"))?;
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

struct RecoveryWorker(Arc<ExcelRecoveryService>);
impl DaemonTask for RecoveryWorker {
    fn run(&self, cancellation: CancellationToken) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            let lane = || async {
                let mut interval = tokio::time::interval(Duration::from_secs(60));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tokio::select! { biased; ()=cancellation.cancelled()=>break, _=interval.tick()=>{} }
                    // Durable database slots, not this process count, enforce the global cap.
                    if self.0.run_one(cancellation.clone()).await.is_err() {
                        tracing::warn!("Excel recovery worker failed; scheduling was not restored");
                    }
                }
            };
            futures::future::join_all((0..3).map(|_| lane())).await;
            Ok(())
        })
    }
}
