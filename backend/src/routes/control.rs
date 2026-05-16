//! Executor control endpoints — status + HALT/RESUME.
//!
//! The executor's control watcher (`executor/src/control/mod.rs`) polls a set
//! of Redis keys every ~250ms. The frontend has no direct Redis access, so
//! these handlers are the bridge:
//!
//!   - `GET  /api/control` → current control state (kill-switch, dead-man).
//!   - `POST /api/control` → drive the control keys.
//!
//! HALT reuses the existing `executor:kill_switch` key (block) **and**
//! `executor:cancel_all` (flatten). The watcher consumes `cancel_all` and
//! DELs it within one poll; `kill_switch` stays set until RESUME so no new
//! order gets through in the meantime. cancel_all calls the exchange's
//! account-wide cancel, so *every* resting order on the wallet is cancelled,
//! not just orders this process tracks.

use std::sync::Arc;

use axum::extract::State;
use axum::response::Json;
use libs::redis_client::keys::Executor;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

/// Mirrors `DEADMAN_THRESHOLD_SECS` in `executor/src/control/mod.rs`.
const DEADMAN_THRESHOLD_SECS: i64 = 30;

#[derive(Serialize)]
pub struct ControlStatus {
    /// `executor:kill_switch == "1"` — all non-cancel orders are blocked.
    pub kill_switch: bool,
    /// Backend heartbeat is stale (>30s) → executor has engaged its local
    /// kill-switch. Derived the same way the executor watcher derives it.
    pub deadman_engaged: bool,
    /// Last `executor:backend_heartbeat` value (unix seconds), or 0 if unset.
    pub last_heartbeat_secs: i64,
}

/// Inbound POST body. We accept the raw object and dispatch on `type`
/// ourselves so an unknown/missing action returns a clean 400 with a JSON
/// error the frontend can display, rather than axum's opaque 422 for a
/// failed enum deserialization.
#[derive(Deserialize)]
pub struct ControlBody {
    #[serde(default)]
    pub r#type: String,
}

pub(crate) async fn get_control(
    State(s): State<Arc<AppState>>,
) -> Result<Json<ControlStatus>, ApiError> {
    let mut conn = s.redis.raw();

    let kill: Option<String> = conn
        .get(Executor::KILL_SWITCH)
        .await
        .map_err(|e| ApiError::Redis(format!("GET {}: {e}", Executor::KILL_SWITCH)))?;

    let hb: Option<String> = conn
        .get(Executor::BACKEND_HEARTBEAT)
        .await
        .map_err(|e| ApiError::Redis(format!("GET {}: {e}", Executor::BACKEND_HEARTBEAT)))?;

    let last_heartbeat_secs = hb.as_deref().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
    let deadman_engaged = last_heartbeat_secs > 0
        && (chrono::Utc::now().timestamp() - last_heartbeat_secs) > DEADMAN_THRESHOLD_SECS;

    Ok(Json(ControlStatus {
        kill_switch: kill.as_deref() == Some("1"),
        deadman_engaged,
        last_heartbeat_secs,
    }))
}

pub(crate) async fn post_control(
    State(s): State<Arc<AppState>>,
    Json(body): Json<ControlBody>,
) -> Result<Json<ControlStatus>, ApiError> {
    let mut conn = s.redis.raw();

    match body.r#type.as_str() {
        "halt" => {
            conn.set::<_, _, ()>(Executor::KILL_SWITCH, "1")
                .await
                .map_err(|e| ApiError::Redis(format!("SET {}: {e}", Executor::KILL_SWITCH)))?;
            conn.set::<_, _, ()>(Executor::CANCEL_ALL, "1")
                .await
                .map_err(|e| ApiError::Redis(format!("SET {}: {e}", Executor::CANCEL_ALL)))?;
            tracing::warn!("control: HALT requested via API (kill_switch + cancel_all set)");
        }
        "resume" => {
            conn.del::<_, ()>(Executor::KILL_SWITCH)
                .await
                .map_err(|e| ApiError::Redis(format!("DEL {}: {e}", Executor::KILL_SWITCH)))?;
            tracing::info!("control: RESUME requested via API (kill_switch cleared)");
        }
        "pause" | "shutdown" | "strategy_params" => {
            return Err(ApiError::BadRequest(format!(
                "action '{}' is not implemented — only 'halt' and 'resume' are supported",
                body.r#type
            )));
        }
        other => {
            return Err(ApiError::BadRequest(format!(
                "unknown control action '{other}' — expected 'halt' or 'resume'"
            )));
        }
    }

    get_control(State(s)).await
}
