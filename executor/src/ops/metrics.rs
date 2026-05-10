//! Prometheus metrics + axum `/metrics` endpoint on `:5558`.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{Router, response::IntoResponse, routing::get};
use prometheus::{
    Encoder, Histogram, HistogramOpts, IntCounterVec, IntGauge, IntGaugeVec, Opts, Registry,
    TextEncoder,
};
use tracing::info;

pub struct Metrics {
    pub registry: Registry,
    pub requests_total: IntCounterVec,
    pub request_latency_seconds: Histogram,
    pub inflight: IntGaugeVec,
    pub kill_switch: IntGaugeVec,
    pub circuit_state: IntGaugeVec,
    pub audit_lag_seconds: IntGauge,
    pub open_orders: IntGaugeVec,
    pub risk_rejections_total: IntCounterVec,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();
        let requests_total = IntCounterVec::new(
            Opts::new("executor_requests_total", "Order requests by outcome"),
            &["exchange", "action", "outcome"],
        )
        .unwrap();
        let request_latency_seconds = Histogram::with_opts(HistogramOpts::new(
            "executor_request_latency_seconds",
            "Request latency seconds",
        ))
        .unwrap();
        let inflight = IntGaugeVec::new(
            Opts::new("executor_inflight", "In-flight requests"),
            &["exchange"],
        )
        .unwrap();
        let kill_switch = IntGaugeVec::new(
            Opts::new("executor_kill_switch", "Kill switch state (1=on)"),
            &["scope"],
        )
        .unwrap();
        let circuit_state = IntGaugeVec::new(
            Opts::new(
                "executor_circuit_state",
                "Circuit breaker state (0=closed, 1=open, 2=half_open)",
            ),
            &["exchange"],
        )
        .unwrap();
        let audit_lag_seconds = IntGauge::with_opts(Opts::new(
            "executor_audit_lag_seconds",
            "Audit queue lag in seconds",
        ))
        .unwrap();
        let open_orders = IntGaugeVec::new(
            Opts::new("executor_open_orders", "Open order count"),
            &["exchange", "symbol"],
        )
        .unwrap();
        let risk_rejections_total = IntCounterVec::new(
            Opts::new("executor_risk_rejections_total", "Risk rejections by rule"),
            &["rule"],
        )
        .unwrap();

        registry.register(Box::new(requests_total.clone())).unwrap();
        registry
            .register(Box::new(request_latency_seconds.clone()))
            .unwrap();
        registry.register(Box::new(inflight.clone())).unwrap();
        registry.register(Box::new(kill_switch.clone())).unwrap();
        registry.register(Box::new(circuit_state.clone())).unwrap();
        registry
            .register(Box::new(audit_lag_seconds.clone()))
            .unwrap();
        registry.register(Box::new(open_orders.clone())).unwrap();
        registry
            .register(Box::new(risk_rejections_total.clone()))
            .unwrap();

        Self {
            registry,
            requests_total,
            request_latency_seconds,
            inflight,
            kill_switch,
            circuit_state,
            audit_lag_seconds,
            open_orders,
            risk_rejections_total,
        }
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

async fn metrics_handler(state: axum::extract::State<Arc<Metrics>>) -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let mut buf = Vec::new();
    if let Err(e) = encoder.encode(&state.registry.gather(), &mut buf) {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("encode error: {e}"),
        )
            .into_response();
    }
    (
        axum::http::StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4")],
        buf,
    )
        .into_response()
}

pub async fn serve(metrics: Arc<Metrics>, addr: SocketAddr) -> anyhow::Result<()> {
    let router = Router::new()
        .route("/metrics", get(metrics_handler))
        .with_state(metrics);
    info!("metrics: listening on http://{}/metrics", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_round_trip() {
        let m = Metrics::new();
        m.requests_total
            .with_label_values(&["kalshi", "place", "ok"])
            .inc();
        let encoder = TextEncoder::new();
        let mut buf = Vec::new();
        encoder.encode(&m.registry.gather(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("executor_requests_total"));
        assert!(s.contains(r#"exchange="kalshi""#));
    }
}
