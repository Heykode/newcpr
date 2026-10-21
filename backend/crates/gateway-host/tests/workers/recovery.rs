use super::*;
use gateway_core::task::WorkerDaemonContext;

struct ControlledDaemon {
    contexts: Arc<Mutex<Vec<WorkerDaemonContext>>>,
    commands: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<DaemonCommand>>,
}

enum DaemonCommand {
    Succeed,
    Fail,
    SucceedThenFail,
    Panic,
    Exit,
}

impl DaemonTask for ControlledDaemon {
    fn run(&self, context: WorkerDaemonContext) -> BoxFuture<'_, Result<(), WorkerTaskError>> {
        Box::pin(async move {
            lock_unpoisoned(&self.contexts).push(context.clone());
            let mut commands = self.commands.lock().await;
            loop {
                let command = tokio::select! {
                    () = context.cancellation().cancelled() => return Ok(()),
                    command = commands.recv() => command.expect("daemon command"),
                };
                match command {
                    DaemonCommand::Succeed => context.succeeded(),
                    DaemonCommand::SucceedThenFail => {
                        context.succeeded();
                        return Err(WorkerTaskError::safe("test daemon failure"));
                    }
                    DaemonCommand::Fail => {
                        return Err(WorkerTaskError::safe("test daemon failure"));
                    }
                    DaemonCommand::Panic => panic!("test daemon panic"),
                    DaemonCommand::Exit => return Ok(()),
                }
            }
        })
    }
}

fn daemon_plan(task: impl DaemonTask + 'static) -> Vec<WorkerContribution> {
    complete_plan(vec![WorkerContribution::Registration(
        WorkerRegistration::try_new(
            WorkerId::try_new(WorkerKind::Backup, "backup").unwrap(),
            WorkerRunnable::Daemon {
                restart: DaemonRestartPolicy::try_new(
                    Duration::from_millis(10),
                    Duration::from_millis(20),
                )
                .unwrap(),
                task: Box::new(task),
            },
        )
        .unwrap(),
    )])
}

#[tokio::test(start_paused = true)]
async fn daemon_recovers_only_after_current_run_succeeds_and_resets_backoff() {
    let (commands, receiver) = tokio::sync::mpsc::unbounded_channel();
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let supervisor = supervisor();
    let id = WorkerId::try_new(WorkerKind::Backup, "backup").unwrap();
    supervisor
        .start(
            daemon_plan(ControlledDaemon {
                contexts: contexts.clone(),
                commands: tokio::sync::Mutex::new(receiver),
            }),
            Arc::new(FakeLeasePort::default()),
        )
        .unwrap();
    yield_until(|| lock_unpoisoned(&contexts).len() == 1).await;

    // 连续失败逐步退避；仅重新进入 run 不能恢复健康
    for (attempt, delay) in [(1, 10), (2, 20)] {
        commands.send(DaemonCommand::Fail).unwrap();
        yield_until(|| task_health(&supervisor, &id).state == WorkerRuntimeState::BackingOff).await;
        assert_eq!(task_health(&supervisor, &id).consecutive_failures, attempt);
        tokio::time::advance(Duration::from_millis(delay - 1)).await;
        assert_eq!(lock_unpoisoned(&contexts).len(), attempt as usize);
        tokio::time::advance(Duration::from_millis(1)).await;
        yield_until(|| lock_unpoisoned(&contexts).len() == attempt as usize + 1).await;
        assert_eq!(task_health(&supervisor, &id).consecutive_failures, attempt);
    }
    lock_unpoisoned(&contexts)[0].succeeded();
    tokio::task::yield_now().await;
    assert_eq!(task_health(&supervisor, &id).consecutive_failures, 2);

    commands.send(DaemonCommand::Succeed).unwrap();
    yield_until(|| task_health(&supervisor, &id).consecutive_failures == 0).await;
    let recovered = task_health(&supervisor, &id);
    assert_eq!(recovered.state, WorkerRuntimeState::Running);
    assert_eq!(recovered.completed_cycles, 1);
    assert!(recovered.last_error.is_none());
    assert!(recovered.last_success_at.is_some());
    assert!(recovered.last_failure_at.is_some());

    commands.send(DaemonCommand::Fail).unwrap();
    yield_until(|| task_health(&supervisor, &id).state == WorkerRuntimeState::BackingOff).await;
    assert_eq!(task_health(&supervisor, &id).consecutive_failures, 1);
    tokio::time::advance(Duration::from_millis(10)).await;
    yield_until(|| lock_unpoisoned(&contexts).len() == 4).await;
    commands.send(DaemonCommand::Fail).unwrap();
    yield_until(|| task_health(&supervisor, &id).state == WorkerRuntimeState::BackingOff).await;
    tokio::time::advance(Duration::from_millis(20)).await;
    yield_until(|| lock_unpoisoned(&contexts).len() == 5).await;

    // 同一 poll 中先成功再退出，必须保留最新故障并重新从初始退避开始
    commands.send(DaemonCommand::SucceedThenFail).unwrap();
    yield_until(|| task_health(&supervisor, &id).state == WorkerRuntimeState::BackingOff).await;
    assert_eq!(task_health(&supervisor, &id).consecutive_failures, 1);
    assert_eq!(task_health(&supervisor, &id).completed_cycles, 2);
    tokio::time::advance(Duration::from_millis(10)).await;
    yield_until(|| lock_unpoisoned(&contexts).len() == 6).await;
    supervisor.shutdown(Duration::from_secs(1)).await;
    lock_unpoisoned(&contexts)[5].succeeded();
    assert_eq!(
        task_health(&supervisor, &id).state,
        WorkerRuntimeState::Stopped
    );
}

#[tokio::test]
async fn daemon_success_remains_healthy_while_idle_beyond_restart_backoff() {
    let (commands, receiver) = tokio::sync::mpsc::unbounded_channel();
    let supervisor = supervisor();
    let id = WorkerId::try_new(WorkerKind::Backup, "backup").unwrap();
    supervisor
        .start(
            daemon_plan(ControlledDaemon {
                contexts: Arc::default(),
                commands: tokio::sync::Mutex::new(receiver),
            }),
            Arc::new(FakeLeasePort::default()),
        )
        .unwrap();
    commands.send(DaemonCommand::Succeed).unwrap();
    wait_until(|| task_health(&supervisor, &id).completed_cycles == 1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let health = task_health(&supervisor, &id);
    assert_eq!(health.state, WorkerRuntimeState::Running);
    assert_eq!(health.consecutive_failures, 0);
    supervisor.shutdown(Duration::from_secs(1)).await;
}

#[tokio::test(start_paused = true)]
async fn daemon_panic_and_unexpected_exit_remain_unhealthy_until_success() {
    for (command, error) in [
        (DaemonCommand::Panic, "daemon panicked"),
        (DaemonCommand::Exit, "daemon exited unexpectedly"),
    ] {
        let (commands, receiver) = tokio::sync::mpsc::unbounded_channel();
        let contexts = Arc::new(Mutex::new(Vec::new()));
        let supervisor = supervisor();
        let id = WorkerId::try_new(WorkerKind::Backup, "backup").unwrap();
        supervisor
            .start(
                daemon_plan(ControlledDaemon {
                    contexts: contexts.clone(),
                    commands: tokio::sync::Mutex::new(receiver),
                }),
                Arc::new(FakeLeasePort::default()),
            )
            .unwrap();
        commands.send(command).unwrap();
        yield_until(|| task_health(&supervisor, &id).state == WorkerRuntimeState::BackingOff).await;
        assert_eq!(
            task_health(&supervisor, &id).last_error.as_deref(),
            Some(error)
        );
        tokio::time::advance(Duration::from_millis(10)).await;
        yield_until(|| lock_unpoisoned(&contexts).len() == 2).await;
        assert_eq!(task_health(&supervisor, &id).consecutive_failures, 1);
        commands.send(DaemonCommand::Succeed).unwrap();
        yield_until(|| task_health(&supervisor, &id).consecutive_failures == 0).await;
        supervisor.shutdown(Duration::from_secs(1)).await;
    }
}
