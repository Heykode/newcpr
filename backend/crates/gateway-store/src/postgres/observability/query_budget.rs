//! 管理观测查询的 PostgreSQL 连接槽位预算。

use std::{
    future::Future,
    ops::{Deref, DerefMut},
    sync::Arc,
    time::{Duration, Instant},
};

use futures::{Stream, TryStreamExt, stream::BoxStream};
use sqlx::{PgConnection, PgPool, Postgres, pool::PoolConnection};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{StoreBackend, StoreError, StoreResult, postgres_unavailable};

const EXECUTION_TIMEOUT: Duration = Duration::from_secs(10);

/// 所有管理观测入口共享的 PostgreSQL 连接槽位预算。
#[derive(Clone)]
pub struct ObservabilityQueryBudget {
    slots: Arc<Semaphore>,
    wait_timeout: Duration,
}

/// 查询取消后的协议收尾和事务回滚仍占用同一个观测名额。
pub struct ObservabilityConnection {
    connection: PoolConnection<Postgres>,
    permit: Option<OwnedSemaphorePermit>,
}

impl Deref for ObservabilityConnection {
    type Target = PgConnection;

    fn deref(&self) -> &Self::Target {
        &self.connection
    }
}

impl DerefMut for ObservabilityConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.connection
    }
}

impl Drop for ObservabilityConnection {
    fn drop(&mut self) {
        let returning = self.connection.return_to_pool();
        let permit = self.permit.take();
        tokio::spawn(async move {
            returning.await;
            drop(permit);
        });
    }
}

impl ObservabilityQueryBudget {
    /// 预算表示观测类 SQL 最多可同时占用的连接数。
    pub fn try_new(max_connections: u32, wait_timeout: Duration) -> StoreResult<Self> {
        if max_connections == 0 || wait_timeout.is_zero() {
            return Err(StoreError::InvalidData {
                entity: "observability query budget",
                message: "requires a positive connection count and wait timeout".to_owned(),
            });
        }
        let permits = usize::try_from(max_connections).map_err(|_| StoreError::InvalidData {
            entity: "observability query budget",
            message: "connection budget does not fit this platform".to_owned(),
        })?;
        Ok(Self {
            slots: Arc::new(Semaphore::new(permits)),
            wait_timeout,
        })
    }

    /// 在共享管理观测预算内执行一条 PostgreSQL 查询。
    ///
    /// # Errors
    ///
    /// 预算在等待期限内没有空闲槽位、预算已关闭，或查询本身失败时返回错误。
    pub async fn run<T, F, Q>(
        &self,
        pool: &PgPool,
        operation: &'static str,
        query: F,
    ) -> StoreResult<T>
    where
        F: FnOnce(ObservabilityConnection) -> Q,
        Q: Future<Output = StoreResult<T>>,
    {
        let permit = self.acquire(operation).await?;
        tokio::time::timeout(EXECUTION_TIMEOUT, async {
            let connection = self.connect(pool, permit).await?;
            query(connection).await
        })
        .await
        .map_err(execution_timeout)?
    }

    /// 将槽位持有到查询流消费结束、报错或被丢弃，而不是仅持有到流创建完成。
    pub fn run_stream<'a, T, F, S>(
        &'a self,
        pool: &'a PgPool,
        operation: &'static str,
        query: F,
    ) -> BoxStream<'a, StoreResult<T>>
    where
        T: Send + 'a,
        F: FnOnce(ObservabilityConnection) -> S + Send + 'a,
        S: Stream<Item = StoreResult<T>> + Send + 'a,
    {
        Box::pin(async_stream::stream! {
            let permit = match self.acquire(operation).await {
                Ok(permit) => permit,
                Err(error) => {
                    yield Err(error);
                    return;
                }
            };
            let deadline = tokio::time::Instant::now() + EXECUTION_TIMEOUT;
            let connection = match tokio::time::timeout_at(deadline, self.connect(pool, permit)).await {
                Ok(Ok(connection)) => connection,
                result => {
                    yield Err(match result {
                        Ok(Err(error)) => error,
                        Err(error) => execution_timeout(error),
                        Ok(Ok(_)) => unreachable!(),
                    });
                    return;
                }
            };
            let mut query = Box::pin(query(connection));
            loop {
                match tokio::time::timeout_at(deadline, query.try_next()).await {
                    Ok(Ok(Some(item))) => yield Ok(item),
                    Ok(Ok(None)) => break,
                    result => {
                        // 返回错误后调用方可能不再 poll，但仍保留流对象。
                        // 必须在交付错误之前释放数据库流和预算。
                        drop(query);
                        yield Err(match result {
                            Ok(Err(error)) => error,
                            Err(error) => execution_timeout(error),
                            Ok(Ok(_)) => unreachable!(),
                        });
                        return;
                    }
                }
            }
        })
    }

    async fn connect(
        &self,
        pool: &PgPool,
        permit: OwnedSemaphorePermit,
    ) -> StoreResult<ObservabilityConnection> {
        let connection = pool.acquire().await.map_err(|error| {
            postgres_unavailable("acquire observability connection").with_source(error)
        })?;
        Ok(ObservabilityConnection {
            connection,
            permit: Some(permit),
        })
    }

    async fn acquire(&self, operation: &'static str) -> StoreResult<OwnedSemaphorePermit> {
        let started_at = Instant::now();
        let acquisition = Arc::clone(&self.slots).acquire_owned();
        match tokio::time::timeout(self.wait_timeout, acquisition).await {
            Ok(Ok(permit)) => {
                tracing::debug!(
                    query_class = "observability",
                    operation,
                    wait_milliseconds = elapsed_milliseconds(started_at),
                    available_slots = self.slots.available_permits(),
                    "PostgreSQL query budget acquired"
                );
                Ok(permit)
            }
            Ok(Err(error)) => Err(StoreError::Unavailable {
                source: Some(error.into()),
                backend: StoreBackend::PostgreSql,
                message: "observability PostgreSQL connection budget is closed".to_owned(),
            }),
            Err(error) => {
                tracing::warn!(
                    query_class = "observability",
                    operation,
                    wait_milliseconds = elapsed_milliseconds(started_at),
                    available_slots = self.slots.available_permits(),
                    "PostgreSQL query budget exhausted"
                );
                Err(StoreError::Unavailable {
                    source: Some(error.into()),
                    backend: StoreBackend::PostgreSql,
                    message: "observability PostgreSQL connection budget is exhausted".to_owned(),
                })
            }
        }
    }
}

fn execution_timeout(error: tokio::time::error::Elapsed) -> StoreError {
    postgres_unavailable("observability PostgreSQL execution budget is exhausted")
        .with_source(error)
}

fn elapsed_milliseconds(started_at: Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
}
