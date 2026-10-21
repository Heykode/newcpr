use crate::{model::AdminError, use_case::notifications::NotificationsService};
use futures::future::BoxFuture;
use gateway_core::task::{
    DaemonRestartPolicy, DaemonTask, WorkerContribution, WorkerId, WorkerKind, WorkerRegistration,
    WorkerRunnable, WorkerTaskError,
};
use std::{sync::Arc, time::Duration};

const INTERVAL: Duration = Duration::from_secs(5);

pub(crate) fn contribution(
    service: Arc<dyn NotificationsService>,
) -> Result<WorkerContribution, AdminError> {
    let id = WorkerId::try_new(
        WorkerKind::RuntimeSnapshotReconciliation,
        "admin_notification_delivery",
    )
    .map_err(|_| AdminError::internal("notification worker ID is invalid"))?;
    let restart = DaemonRestartPolicy::try_new(INTERVAL, INTERVAL)
        .map_err(|_| AdminError::internal("notification worker policy is invalid"))?;
    Ok(WorkerContribution::Registration(
        WorkerRegistration::try_new(
            id,
            WorkerRunnable::Daemon {
                restart,
                task: Box::new(NotificationDispatcher { service }),
            },
        )
        .map_err(|_| AdminError::internal("notification worker registration is invalid"))?,
    ))
}

struct NotificationDispatcher {
    service: Arc<dyn NotificationsService>,
}

impl DaemonTask for NotificationDispatcher {
    fn run(
        &self,
        context: gateway_core::task::WorkerDaemonContext,
    ) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            let cancellation = context.cancellation().clone();
            let mut interval = tokio::time::interval(INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! { biased; () = cancellation.cancelled() => return Ok(()), _ = interval.tick() => {} }
                for _ in 0..16 {
                    if cancellation.is_cancelled() {
                        return Ok(());
                    }
                    match self.service.dispatch_one().await {
                        Ok(true) => context.succeeded(),
                        Ok(false) => {
                            context.succeeded();
                            break;
                        }
                        Err(error) => {
                            tracing::warn!(error = %error, "notification dispatcher cycle failed");
                            break;
                        }
                    }
                }
            }
        })
    }
}
