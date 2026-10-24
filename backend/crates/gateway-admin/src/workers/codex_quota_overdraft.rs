use std::{sync::Arc, time::Duration};

use futures::future::BoxFuture;
use gateway_core::task::{
    DaemonRestartPolicy, DaemonTask, WorkerContribution, WorkerId, WorkerKind, WorkerRegistration,
    WorkerRunnable, WorkerTaskError,
};

use crate::{model::AdminError, use_case::codex_quota_overdraft::CodexQuotaOverdraftService};

pub(crate) fn contribution(
    service: Arc<CodexQuotaOverdraftService>,
) -> Result<WorkerContribution, AdminError> {
    let id = WorkerId::try_new(
        WorkerKind::RuntimeSnapshotReconciliation,
        "admin_codex_quota_overdraft",
    )
    .map_err(|_| AdminError::internal("invalid Codex quota overdraft worker ID"))?;
    let interval = Duration::from_secs(5);
    let restart = DaemonRestartPolicy::try_new(interval, Duration::from_secs(30))
        .map_err(|_| AdminError::internal("invalid Codex quota overdraft worker policy"))?;
    let registration = WorkerRegistration::try_new(
        id,
        WorkerRunnable::Daemon {
            restart,
            task: Box::new(CodexQuotaOverdraftWorker { service }),
        },
    )
    .map_err(|_| AdminError::internal("invalid Codex quota overdraft worker"))?;
    Ok(WorkerContribution::Registration(registration))
}

struct CodexQuotaOverdraftWorker {
    service: Arc<CodexQuotaOverdraftService>,
}

impl DaemonTask for CodexQuotaOverdraftWorker {
    fn run(
        &self,
        context: gateway_core::task::WorkerDaemonContext,
    ) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            let cancellation = context.cancellation().clone();
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    () = cancellation.cancelled() => break,
                    _ = interval.tick() => {}
                }
                match self.service.run_one(cancellation.clone()).await {
                    Ok(true) => context.succeeded(),
                    Ok(false) => {}
                    Err(error) => tracing::warn!(%error, "Codex quota overdraft worker failed"),
                }
            }
            Ok(())
        })
    }
}
