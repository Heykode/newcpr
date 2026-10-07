use super::{PgLogCleanupStore, runner::BatchOutcome};
use futures::future::BoxFuture;
use gateway_admin::ports::store::AdminStoreResult;
use gateway_core::{
    lifecycle::CancellationToken,
    task::{DaemonTask, WorkerTaskError},
};
use std::{future::Future, time::Duration};
use tokio::sync::Notify;

const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(2);
const RETRY_INTERVAL: Duration = Duration::from_secs(1);

impl DaemonTask for PgLogCleanupStore {
    fn run(&self, cancellation: CancellationToken) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            run_loop(&self.wakeup, cancellation, || self.run_next_batch())
                .await
                .map_err(|_| WorkerTaskError::safe("retention cleanup failed"))
        })
    }
}

async fn run_loop<F, Fut>(
    wakeup: &Notify,
    cancellation: CancellationToken,
    mut batch: F,
) -> AdminStoreResult<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = AdminStoreResult<BatchOutcome>>,
{
    while !cancellation.is_cancelled() {
        // Finish the bounded batch before shutdown, including progress for file deletions.
        match batch().await? {
            BatchOutcome::Continue => tokio::task::yield_now().await,
            BatchOutcome::Idle => {
                tokio::select! {
                    () = cancellation.cancelled() => break,
                    () = wakeup.notified() => {},
                    () = tokio::time::sleep(IDLE_POLL_INTERVAL) => {},
                }
            }
            BatchOutcome::Retry => {
                // Notifications must not turn a contended batch into a busy loop.
                tokio::select! {
                    () = cancellation.cancelled() => break,
                    () = tokio::time::sleep(RETRY_INTERVAL) => {},
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
