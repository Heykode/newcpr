# Daemon Health Recovery

Adapt upstream #499 without changing task intervals or scheduling authority.
`DaemonTask::run` receives a fresh `WorkerDaemonContext` for each supervised run.
Successful work or dependency checks call `succeeded`; merely spawning a task is
not recovery. The context combines cancellation and a monotonic progress counter.

The host consumes progress before recording a simultaneous exit, resets previous
failure/backoff after success, and leaves an active daemon in `Running`. It cancels
the context at exit, so stale generation reports cannot repair a newer failure.
Register the progress listener before comparing counters to avoid lost wakeups.

Daemon restart backoff is not a work freshness deadline. Long idle periods and
backups must not become unhealthy solely because that duration passed. Scheduled
tasks retain their existing freshness, lease, fencing and cancellation contracts.
Custom cleanup and observation writers report success only on completed writes or
successful dependency checks, not failed or expired cleanup attempts.

Keep regressions for successful restart recovery, initial backoff restoration,
stale generation isolation and long-idle daemon health. API health serialization
and ordinary request scheduling remain unchanged.
