//! Host-supervised identity reconciliation and group monitor sampling.

use std::{sync::Arc, time::Duration};

use futures::future::BoxFuture;
use gateway_core::{
    routing::ProviderKind,
    task::{
        ScheduledTask, WorkerContribution, WorkerCycleContext, WorkerId, WorkerKind,
        WorkerRegistration, WorkerRunnable, WorkerSchedule, WorkerTaskError,
    },
};

use crate::{model::AdminError, use_case::user_agent::DefaultOutboundUserAgentService};

const CYCLE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) mod group_monitor;
pub(crate) mod notifications;

pub(crate) fn user_agent_reconciliation(
    service: Arc<DefaultOutboundUserAgentService>,
    providers: Vec<ProviderKind>,
) -> Result<WorkerContribution, AdminError> {
    let id = WorkerId::try_new(
        WorkerKind::RuntimeSnapshotReconciliation,
        "admin_user_agent",
    )
    .map_err(|_| AdminError::internal("outbound user-agent worker ID is invalid"))?;
    let schedule = WorkerSchedule::try_new(
        Duration::from_secs(5),
        Duration::from_secs(1),
        Duration::from_secs(5),
        Duration::from_secs(30),
        Duration::from_secs(10),
    )
    .map_err(|_| AdminError::internal("outbound user-agent worker schedule is invalid"))?;
    let registration = WorkerRegistration::try_new(
        id,
        WorkerRunnable::Scheduled {
            schedule,
            // Every process must refresh its own runtime projection.
            lease: None,
            task: Box::new(UserAgentReconciliation { service, providers }),
        },
    )
    .map_err(|_| AdminError::internal("outbound user-agent worker registration is invalid"))?;
    Ok(WorkerContribution::Registration(registration))
}

struct UserAgentReconciliation {
    service: Arc<DefaultOutboundUserAgentService>,
    providers: Vec<ProviderKind>,
}

impl ScheduledTask for UserAgentReconciliation {
    fn run_cycle(&self, context: WorkerCycleContext) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            let reconcile = async {
                let mut failed = false;
                for provider in &self.providers {
                    failed |= self.service.reconcile(provider).await.is_err();
                }
                if failed {
                    Err(WorkerTaskError::safe(
                        "outbound user-agent reconciliation failed",
                    ))
                } else {
                    Ok(())
                }
            };
            tokio::select! {
                biased;
                () = context.cancellation().cancelled() => Ok(()),
                result = tokio::time::timeout(CYCLE_TIMEOUT, reconcile) => {
                    result.unwrap_or_else(|_| Err(WorkerTaskError::safe(
                        "outbound user-agent reconciliation timed out",
                    )))
                }
            }
        })
    }
}
pub(crate) mod token_guard;
