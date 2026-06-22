//! `Executor` — orchestrator that owns all per-request state and
//! implements every capability trait. The ZMQ `Gateway` becomes a thin
//! adapter that calls [`Executor::handle_one`].
//!
//! Per-request flow (straight-line, named helpers — see Plan §A):
//!
//! ```text
//!   decode → log_request → idem_lookup
//!         → kill_switch?  → pretrade?
//!         → dispatch
//!         → update_registry + update_open_orders
//!         → log_response  + idem_stash
//! ```
//!
//! Pretrade is the **only** subsystem that can reject on risk grounds.
//! Everything else around dispatch is state-maintenance.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use libs::protocol::orders::{
    OrderAction, OrderError, OrderRequest, OrderResponse, OrderResult, OrderStatus,
};
use libs::protocol::ExchangeName;
use libs::redis_client::RedisHandle;
use tracing::warn;

use crate::control::{ControlCallbacks, ControlState, ShutdownState};
use crate::gateway::ClientMap;
use crate::ops::{AuditSink, Metrics};
use crate::pretrade::{PretradeEngine, PretradeState};
use crate::registry::OrderRegistry;
use crate::rest::RestOrderClient;
use crate::traits::{Audit, Dispatch, Gated, Idempotent, Meter, Registry, Risk};

/// Everything the executor needs at construction time.
pub struct ExecutorBuild {
    pub clients: ClientMap,
    pub audit: Arc<AuditSink>,
    pub metrics: Arc<Metrics>,
    pub control: Arc<ControlState>,
    pub shutdown: Arc<ShutdownState>,
    pub redis: Option<RedisHandle>,
    pub config_path: PathBuf,
}

pub struct Executor {
    clients: ClientMap,
    audit: Arc<AuditSink>,
    metrics: Arc<Metrics>,
    control: Arc<ControlState>,
    shutdown: Arc<ShutdownState>,
    pretrade: PretradeEngine,
    pretrade_state: PretradeState,
    registry: OrderRegistry,
    redis: Option<RedisHandle>,
    /// Path the risk gate is (re)loaded from — the sibling `risk.yaml` next to
    /// the `--config` file (e.g. `configs/<label>/config.yaml` → `…/risk.yaml`).
    risk_config_path: PathBuf,
}

/// The risk file lives alongside the main config: `<config dir>/risk.yaml`.
/// Both startup seeding and hot-reload read from here, so they always agree.
pub fn risk_path_for(config_path: &std::path::Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("risk.yaml")
}

impl Executor {
    pub fn new(b: ExecutorBuild) -> Self {
        Self {
            clients: b.clients,
            audit: b.audit,
            metrics: b.metrics,
            control: b.control,
            shutdown: b.shutdown,
            pretrade: PretradeEngine::default(),
            pretrade_state: PretradeState::default(),
            registry: OrderRegistry::default(),
            redis: b.redis,
            risk_config_path: risk_path_for(&b.config_path),
        }
    }

    /// Path the risk gate loads from (sibling `risk.yaml`). Used by the binary
    /// to seed the gate at startup before the first hot-reload.
    pub fn risk_config_path(&self) -> &std::path::Path {
        &self.risk_config_path
    }

    /// Load the risk config from `risk_config_path` and swap it into the gate.
    /// Fail-soft: on a missing/invalid file the current limits are kept (at
    /// startup that's the default = gating disabled), matching prior behavior.
    pub fn load_risk_config(&self) {
        let path_str = self.risk_config_path.to_string_lossy().to_string();
        match libs::configs::load_yaml::<libs::configs::RiskConfig, _>(&path_str) {
            Ok(risk) => {
                let enabled = risk.enabled;
                self.pretrade_state.swap(risk);
                tracing::info!("executor: risk config loaded from {path_str} (enabled={enabled})");
            }
            Err(e) => {
                tracing::warn!(
                    "executor: risk config not loaded from {path_str} ({e}); gating uses current limits"
                );
            }
        }
    }

    /// Exposed for the boot-time reconcile in `bin/executor.rs`.
    pub fn registry(&self) -> &OrderRegistry {
        &self.registry
    }
    pub fn clients(&self) -> &ClientMap {
        &self.clients
    }
    pub fn audit(&self) -> &Arc<AuditSink> {
        &self.audit
    }
    pub fn control(&self) -> &Arc<ControlState> {
        &self.control
    }
    pub fn shutdown(&self) -> &Arc<ShutdownState> {
        &self.shutdown
    }
    pub fn exchanges(&self) -> Vec<ExchangeName> {
        self.clients.keys().copied().collect()
    }

    /// One-shot boot path: replay the on-disk audit log to repopulate the
    /// `OrderRegistry` with anything we placed before the restart.
    pub async fn rehydrate_registry(&self, audit_dir: &std::path::Path) {
        self.registry.rehydrate_from_audit(audit_dir).await;
    }

    /// Per-request orchestrator. Option-C straight-line, with each step
    /// as a named helper on `&self`.
    ///
    /// **TEMPORARY (order-placement debugging):** every gate + bookkeeping
    /// step is commented out so the only thing between the wire and the
    /// exchange REST client is `decode → dispatch`. Restore the full
    /// pipeline before merging — see the original ordering in steps 2–7.
    pub async fn handle_request(&self, payload: &[u8]) -> OrderResponse {
        // 1. Decode.
        let req: OrderRequest = match rmp_serde::from_slice(payload) {
            Ok(r) => r,
            Err(e) => {
                warn!("executor: decode failed: {e}");
                return OrderResponse {
                    req_id: 0,
                    result: Err(OrderError::Internal {
                        message: format!("decode: {e}"),
                    }),
                };
            }
        };

        // // 2. Audit request (always — even rejected paths leave a trace).
        // self.audit_request(&req).await;

        // // 3. Idempotency replay.
        // if let Some(cached) = self.idem_lookup(&req) {
        //     return OrderResponse {
        //         req_id: req.req_id,
        //         result: cached.result,
        //     };
        // }

        // // 4. Operational gate (kill / pause / deadman). Cancels pass.
        // if self.is_blocked(&req) && !is_cancel(&req.action) {
        //     let resp = OrderResponse {
        //         req_id: req.req_id,
        //         result: Err(OrderError::KillSwitch),
        //     };
        //     self.audit_response(req.req_id, &resp.result, true).await;
        //     return resp;
        // }

        // // 5. Pre-trade risk pipeline (the only risk rejection point).
        // if let Some((rule, detail)) = self.run_pretrade(&req).await {
        //     self.inc_risk_rejection(rule);
        //     let resp = OrderResponse {
        //         req_id: req.req_id,
        //         result: Err(OrderError::RiskRejected {
        //             rule: rule.to_string(),
        //             detail,
        //         }),
        //     };
        //     self.audit_response(req.req_id, &resp.result, true).await;
        //     self.idem_stash(&req, &resp);
        //     return resp;
        // }

        // 6. Dispatch to the exchange.
        let result = self.dispatch(&req).await;
        let resp = OrderResponse {
            req_id: req.req_id,
            result: result.clone(),
        };

        // // 7. State maintenance (always — even on dispatch errors so the
        // //    registry/audit can witness the failure).
        // if result.is_ok() {
        //     self.update_open_orders(&req);
        // }
        // self.update_registry(&req, &result);
        // let critical = matches!(
        //     &result,
        //     Err(OrderError::ExchangeRejected { .. } | OrderError::Internal { .. })
        // );
        // self.audit_response(req.req_id, &result, critical).await;
        // self.idem_stash(&req, &resp);

        let _ = result; // silence unused-result warning in the bypass mode.
        resp
    }

    /// Best-effort open-order counting + gauge update.
    #[allow(dead_code)] // bypass: re-enable when handle_request restores step 7
    fn update_open_orders(&self, req: &OrderRequest) {
        let ex = req.exchange;
        match &req.action {
            OrderAction::Place(p) => {
                self.pretrade_state.incr_open(ex, &p.symbol);
                self.set_open_orders(
                    ex.to_str(),
                    &p.symbol,
                    self.pretrade_state.open_count(ex, &p.symbol) as i64,
                );
            }
            OrderAction::PlaceBatch { orders } => {
                for p in orders {
                    self.pretrade_state.incr_open(ex, &p.symbol);
                    self.set_open_orders(
                        ex.to_str(),
                        &p.symbol,
                        self.pretrade_state.open_count(ex, &p.symbol) as i64,
                    );
                }
            }
            OrderAction::CancelMarket { symbol } => {
                self.pretrade_state.decr_open(ex, symbol);
                self.set_open_orders(
                    ex.to_str(),
                    symbol,
                    self.pretrade_state.open_count(ex, symbol) as i64,
                );
            }
            OrderAction::CancelAll => {
                self.pretrade_state.reset_open_for(ex);
                self.registry.reset_for_exchange(ex);
            }
            // Single Cancel by exchange_oid: we don't track oid→symbol;
            // count self-corrects via cancel-all / restart. Intentional.
            _ => {}
        }
    }
}

pub fn is_cancel(a: &OrderAction) -> bool {
    matches!(
        a,
        OrderAction::Cancel { .. } | OrderAction::CancelAll | OrderAction::CancelMarket { .. }
    )
}

// ───── trait impls ──────────────────────────────────────────────────────────

#[async_trait]
impl Audit for Executor {
    async fn audit_request(&self, req: &OrderRequest) {
        self.audit.log_request(req).await;
    }
    async fn audit_response(
        &self,
        req_id: u64,
        result: &Result<OrderResult, OrderError>,
        critical: bool,
    ) {
        self.audit.log_response(req_id, result, critical).await;
    }
}

impl Idempotent for Executor {
    fn idem_lookup(&self, req: &OrderRequest) -> Option<OrderResponse> {
        let key = match &req.action {
            OrderAction::Place(p) => &p.client_oid,
            // PlaceBatch is treated as one transaction keyed by the
            // first leg's client_oid (same as the old gateway).
            OrderAction::PlaceBatch { orders } => orders.first().map(|o| &o.client_oid)?,
            _ => return None,
        };
        self.registry.lookup_idem(key)
    }
    fn idem_stash(&self, req: &OrderRequest, response: &OrderResponse) {
        // PlaceBatch is recorded leg-by-leg inside `record`, so we just
        // pass through.
        self.registry.record(req, response);
    }
}

impl Registry for Executor {
    fn update_registry(&self, _req: &OrderRequest, result: &Result<OrderResult, OrderError>) {
        // For terminal Cancel results, mark the targeted entry terminal.
        if let Ok(OrderResult::Ack(ack)) = result {
            if matches!(
                ack.status,
                OrderStatus::Canceled
                    | OrderStatus::Filled
                    | OrderStatus::Rejected
                    | OrderStatus::Expired
            ) {
                self.registry.mark_terminal(&ack.client_oid, ack.status);
            }
        }
    }
}

impl Gated for Executor {
    fn is_blocked(&self, req: &OrderRequest) -> bool {
        self.control.is_blocked(req.exchange)
    }
}

#[async_trait]
impl Dispatch for Executor {
    async fn dispatch(&self, req: &OrderRequest) -> Result<OrderResult, OrderError> {
        let client = match self.clients.get(&req.exchange) {
            Some(c) => c.clone(),
            None => {
                return Err(OrderError::Internal {
                    message: format!("no client configured for {}", req.exchange),
                });
            }
        };
        dispatch_action(&*client, &req.action).await
    }
}

async fn dispatch_action(
    client: &dyn RestOrderClient,
    action: &OrderAction,
) -> Result<OrderResult, OrderError> {
    match action {
        OrderAction::Place(p) => client.place(p).await.map(OrderResult::Ack),
        OrderAction::PlaceBatch { orders } => {
            let results = client.place_batch(orders).await?;
            Ok(OrderResult::BatchAck { results })
        }
        OrderAction::Update {
            client_oid,
            exchange_oid,
            new_px,
            new_qty,
        } => client
            .update(client_oid, exchange_oid, *new_px, *new_qty)
            .await
            .map(OrderResult::Ack),
        OrderAction::Cancel { exchange_oid } => client
            .cancel(exchange_oid)
            .await
            .map(|_ack| OrderResult::CancelCount { count: 1 }),
        OrderAction::CancelAll => client
            .cancel_all()
            .await
            .map(|count| OrderResult::CancelCount { count }),
        OrderAction::CancelMarket { symbol } => client
            .cancel_market(symbol)
            .await
            .map(|count| OrderResult::CancelCount { count }),
        OrderAction::Heartbeat => client.heartbeat().await.map(OrderResult::HeartbeatOk),
    }
}

#[async_trait]
impl Risk for Executor {
    async fn run_pretrade(&self, req: &OrderRequest) -> Option<(&'static str, String)> {
        self.pretrade
            .run(req, &self.pretrade_state, self.redis.as_ref())
            .await
    }
}

impl Meter for Executor {
    fn inc_risk_rejection(&self, rule: &str) {
        self.metrics
            .risk_rejections_total
            .with_label_values(&[rule])
            .inc();
    }
    fn set_open_orders(&self, exchange: &str, symbol: &str, count: i64) {
        self.metrics
            .open_orders
            .with_label_values(&[exchange, symbol])
            .set(count);
    }
}

// ───── control-plane callback bridge ────────────────────────────────────────

/// Wrapper that lets [`crate::control::run_watcher`] call back into the
/// executor without taking a direct `Arc<Executor>` (keeps the control
/// thread genuinely standalone — it only sees a trait).
pub struct ExecutorControlCallbacks {
    pub executor: Arc<Executor>,
}

#[async_trait]
impl ControlCallbacks for ExecutorControlCallbacks {
    async fn cancel_all_now(&self) {
        let exchanges: Vec<_> = self.executor.exchanges();
        for ex in exchanges {
            if let Some(client) = self.executor.clients.get(&ex) {
                match client.cancel_all().await {
                    Ok(n) => {
                        self.executor
                            .audit
                            .log_synthetic(
                                "cancel_all_fired",
                                serde_json::json!({ "exchange": ex.to_str(), "count": n }),
                            )
                            .await;
                    }
                    Err(e) => {
                        self.executor
                            .audit
                            .log_synthetic(
                                "cancel_all_error",
                                serde_json::json!({
                                    "exchange": ex.to_str(),
                                    "error": format!("{e:?}"),
                                }),
                            )
                            .await;
                    }
                }
            }
            self.executor.pretrade_state.reset_open_for(ex);
            self.executor.registry.reset_for_exchange(ex);
        }
    }

    async fn reload_config(&self) {
        // Re-read the dedicated risk file (sibling `risk.yaml`); the rest of the
        // unified config is not hot-reloadable.
        self.executor.load_risk_config();
    }
}

// Silence an unused-import warning if HashSet only used in tests later.
#[allow(dead_code)]
fn _anchor(_: HashSet<String>) {}
