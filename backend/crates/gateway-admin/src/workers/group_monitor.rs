//! One Host-supervised sampler, independent of browser activity.

use std::{sync::Arc, time::Duration};

use futures::future::BoxFuture;
use gateway_core::{
    lifecycle::CancellationToken,
    task::{
        DaemonRestartPolicy, DaemonTask, WorkerContribution, WorkerId, WorkerKind,
        WorkerRegistration, WorkerRunnable, WorkerTaskError,
    },
};

use crate::{GroupMonitorService, model::AdminError};

const INTERVAL: Duration = Duration::from_secs(10);

pub(crate) fn contribution(
    service: Arc<dyn GroupMonitorService>,
) -> Result<WorkerContribution, AdminError> {
    let id = WorkerId::try_new(
        WorkerKind::RuntimeSnapshotReconciliation,
        "admin_group_monitor",
    )
    .map_err(|_| AdminError::internal("group monitor worker ID is invalid"))?;
    let restart = DaemonRestartPolicy::try_new(INTERVAL, INTERVAL)
        .map_err(|_| AdminError::internal("group monitor restart policy is invalid"))?;
    let registration = WorkerRegistration::try_new(
        id,
        WorkerRunnable::Daemon {
            restart,
            task: Box::new(GroupMonitorSampler { service }),
        },
    )
    .map_err(|_| AdminError::internal("group monitor worker registration is invalid"))?;
    Ok(WorkerContribution::Registration(registration))
}

struct GroupMonitorSampler {
    service: Arc<dyn GroupMonitorService>,
}

impl DaemonTask for GroupMonitorSampler {
    fn run(&self, cancellation: CancellationToken) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            let mut interval = tokio::time::interval(INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    () = cancellation.cancelled() => return Ok(()),
                    _ = interval.tick() => {}
                }
                tokio::select! {
                    biased;
                    () = cancellation.cancelled() => return Ok(()),
                    result = self.service.sample() => {
                        if result.is_err() {
                            // Display-only sampling must not fail gateway readiness.
                            tracing::warn!("group monitor sampling failed; previous snapshot retained");
                        }
                    },
                }
            }
        })
    }
}
