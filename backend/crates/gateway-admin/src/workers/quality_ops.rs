use crate::{model::AdminError, use_case::quality_ops::QualityOpsService};
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
    service: Arc<QualityOpsService>,
) -> Result<WorkerContribution, AdminError> {
    let interval = Duration::from_secs(5);
    let id = WorkerId::try_new(
        WorkerKind::RuntimeSnapshotReconciliation,
        "admin_quality_ops",
    )
    .map_err(|_| AdminError::internal("invalid quality worker ID"))?;
    let restart = DaemonRestartPolicy::try_new(interval, interval)
        .map_err(|_| AdminError::internal("invalid quality restart policy"))?;
    let registration = WorkerRegistration::try_new(
        id,
        WorkerRunnable::Daemon {
            restart,
            task: Box::new(QualityWorker { service }),
        },
    )
    .map_err(|_| AdminError::internal("invalid quality worker"))?;
    Ok(WorkerContribution::Registration(registration))
}

struct QualityWorker {
    service: Arc<QualityOpsService>,
}

impl DaemonTask for QualityWorker {
    fn run(&self, cancellation: CancellationToken) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            let lane = || async {
                let mut interval = tokio::time::interval(Duration::from_secs(5));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tokio::select! {
                        biased;
                        () = cancellation.cancelled() => break,
                        _ = interval.tick() => {}
                    }
                    if self.service.run_one(cancellation.clone()).await.is_err() {
                        tracing::warn!("quality check worker failed; ordinary requests unaffected");
                    }
                }
            };
            let cleanup = async {
                let mut interval = tokio::time::interval(Duration::from_secs(3600));
                loop {
                    tokio::select! {
                        biased;
                        () = cancellation.cancelled() => break,
                        _ = interval.tick() => {}
                    }
                    if self.service.cleanup().await.is_err() {
                        tracing::warn!("quality history retention failed");
                    }
                }
            };
            let lanes = futures::future::join_all(
                (0..crate::model::quality_ops::QUALITY_MAX_WORKERS).map(|_| lane()),
            );
            tokio::join!(lanes, cleanup);
            Ok(())
        })
    }
}
