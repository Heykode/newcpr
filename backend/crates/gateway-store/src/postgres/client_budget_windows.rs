//! 在原生预算行锁内设置窗口、重算所属费用并识别最近一次重复操作

use gateway_admin::{
    model::{
        MutationContext,
        client_keys::{
            ClientKeyBudgetWindow, ClientKeyBudgetWindowPeriod, SetClientKeyBudgetWindow,
        },
    },
    ports::store::{AdminStoreError, AdminStoreErrorKind, AdminStoreResult},
};
use gateway_core::{metering::Decimal, policy::ClientApiKeyId};
use sqlx::{PgPool, Row};

use crate::mutation_audit;

fn unavailable(source: impl Into<gateway_core::error::ErrorSource>) -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Unavailable,
        "client API key",
        "预算窗口操作失败",
    )
    .with_source(source)
}

pub(super) async fn get(
    pool: &PgPool,
    id: &ClientApiKeyId,
    period: ClientKeyBudgetWindowPeriod,
) -> AdminStoreResult<ClientKeyBudgetWindow> {
    // 列名前缀只来自封闭枚举，不接受调用方字符串
    let column = period.as_str();
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "select coalesce(w.revision, 0) as revision, false as explicit,
        w.{column}_start as start, w.{column}_end as finish,
        w.{column}_used_usd::text as used
        from client_api_keys k left join client_key_budget_windows w on w.client_api_key_id=k.id where k.id=$1"
    ))).bind(id.as_str()).fetch_optional(pool).await.map_err(unavailable)?
        .ok_or_else(|| AdminStoreError::new(AdminStoreErrorKind::NotFound, "client API key", "API Key 不存在"))?;
    let start = row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("start");
    let end = row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("finish");
    let visible = start
        .zip(end)
        .is_some_and(|(start, end)| (start..end).contains(&chrono::Utc::now()));
    Ok(ClientKeyBudgetWindow {
        revision: row.get::<i64, _>("revision") as u64,
        explicit: false,
        starts_at: start.filter(|_| visible),
        ends_at: end.filter(|_| visible),
        used_usd: if visible {
            row.get::<String, _>("used")
                .parse::<Decimal>()
                .map_err(unavailable)?
        } else {
            Decimal::ZERO
        },
    })
}

pub(super) async fn set(
    pool: &PgPool,
    command: SetClientKeyBudgetWindow,
    context: &MutationContext,
) -> AdminStoreResult<u64> {
    let mut tx = pool.begin().await.map_err(unavailable)?;
    let exists = sqlx::query("select id from client_api_keys where id=$1 for update")
        .bind(command.id.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
    if exists.is_none() {
        return Err(AdminStoreError::new(
            AdminStoreErrorKind::NotFound,
            "client API key",
            "API Key 不存在",
        ));
    }
    let request = serde_json::json!({
        "period": command.period.as_str(),
        "starts_at": command.window.map(|window| window.starts_at),
        "ends_at": command.window.map(|window| window.ends_at),
        "resume_calendar": command.window.is_some_and(|window| window.resume_calendar),
    });
    // 只创建未开启的账本，不因窗口管理顺带开启另一个原生周期
    sqlx::query(
        "insert into client_key_budget_windows
        (client_api_key_id,daily_start,daily_end,weekly_start,weekly_end)
        values ($1,now(),now(),now(),now()) on conflict do nothing",
    )
    .bind(command.id.as_str())
    .execute(&mut *tx)
    .await
    .map_err(unavailable)?;
    let row = sqlx::query("select revision,last_operation_request from client_key_budget_windows where client_api_key_id=$1")
        .bind(command.id.as_str()).fetch_one(&mut *tx).await.map_err(unavailable)?;
    let revision = row.get::<i64, _>("revision") as u64;
    // 版本就是该 Key 的操作标识，只为最近一次提交保留可核对的参数
    if command.expected_revision.checked_add(1) == Some(revision) {
        if row
            .get::<Option<serde_json::Value>, _>("last_operation_request")
            .as_ref()
            != Some(&request)
        {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::Conflict,
                "client API key",
                "该预算版本已用于其他操作",
            ));
        }
        tx.commit().await.map_err(unavailable)?;
        return Ok(revision);
    }
    if revision != command.expected_revision {
        return Err(AdminStoreError::new(
            AdminStoreErrorKind::StaleRevision,
            "client API key",
            "预算窗口已被修改",
        ));
    }
    let column = command.period.as_str();
    let next: i64 = match command.window {
        Some(window) => {
            if !window.resume_calendar
                || !(window.starts_at..window.ends_at).contains(&chrono::Utc::now())
            {
                return Err(AdminStoreError::new(
                    AdminStoreErrorKind::Conflict,
                    "client API key",
                    "参考窗口已失效，请重新选择",
                ));
            }
            // 首次接管和迟到的周期同步都从同一费用账本计入窗口内已完成请求
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "update client_key_budget_windows set
                {column}_start=$2, {column}_end=$3,
                {column}_used_usd=(select coalesce(sum(amount_usd),0) from client_key_charge_events
                    where client_api_key_id=$1 and completed_at >= $2 and completed_at < $3),
                revision=revision+1,last_operation_request=$4
                where client_api_key_id=$1 returning revision"
            )))
            .bind(command.id.as_str())
            .bind(window.starts_at)
            .bind(window.ends_at)
            .bind(request)
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?
        }
        None => {
            // 释放外部控制时保留已用金额与边界，到期后再按原生规则滚动
            sqlx::query_scalar(
                "update client_key_budget_windows set
                revision=revision+1,last_operation_request=$2
                where client_api_key_id=$1 returning revision",
            )
            .bind(command.id.as_str())
            .bind(request)
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?
        }
    };
    super::admin_security_audit::insert_admin_audit_event(
        &mut tx,
        mutation_audit(
            context,
            "set_budget_window",
            "client_api_key",
            command.id.as_str(),
            vec![format!("{column}_window")],
        ),
    )
    .await
    .map_err(|error| crate::admin_store_error("client API key", error))?;
    tx.commit().await.map_err(unavailable)?;
    Ok(next as u64)
}
