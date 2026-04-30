//! Upstream-CEX spot price routes:
//!   - `/api/spot_price/:product`         — always-fresh tick
//!   - `/api/window_open_price/:product/:interval_secs/:window_start`
//!                                         — Redis-cached snapshot, frozen
//!                                           per window so the UI's
//!                                           "price-to-beat" hint doesn't
//!                                           drift mid-window.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    response::Json,
};
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Deserialize)]
pub(crate) struct SpotPriceQuery {
    pub source: Option<String>,
}

/// Fetch the latest spot tick for `product` from one of
/// {coinbase, kraken, binance}. Shared between the on-demand `/spot_price`
/// route and the cached `/window_open_price` route.
pub(crate) async fn fetch_spot(
    http: &reqwest::Client,
    product: &str,
    source: &str,
) -> Result<f64, ApiError> {
    let (url, parse): (String, fn(&serde_json::Value) -> Option<f64>) = match source {
        "coinbase" => (
            format!("https://api.exchange.coinbase.com/products/{product}/ticker"),
            |v| {
                v.get("price")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok())
            },
        ),
        "kraken" => {
            let pair = match product {
                "BTC-USD" => "XBTUSD".to_string(),
                "ETH-USD" => "ETHUSD".to_string(),
                p => p.replace('-', ""),
            };
            (
                format!("https://api.kraken.com/0/public/Ticker?pair={pair}"),
                |v| {
                    v.get("result")
                        .and_then(|r| r.as_object())
                        .and_then(|m| m.values().next())
                        .and_then(|e| e.get("c"))
                        .and_then(|c| c.as_array())
                        .and_then(|a| a.first())
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse().ok())
                },
            )
        }
        "binance" => {
            let symbol = match product {
                "BTC-USD" => "BTCUSDT".to_string(),
                "ETH-USD" => "ETHUSDT".to_string(),
                p => p.replace('-', "").to_uppercase(),
            };
            (
                format!("https://api.binance.com/api/v3/ticker/price?symbol={symbol}"),
                |v| {
                    v.get("price")
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse().ok())
                },
            )
        }
        other => {
            return Err(ApiError::NotFound(format!(
                "unknown source '{other}' (use coinbase|kraken|binance)"
            )));
        }
    };

    let resp = http
        .get(&url)
        .send()
        .await
        .map_err(|e| ApiError::Network(format!("{source}: {e}")))?;
    if !resp.status().is_success() {
        return Err(ApiError::Network(format!(
            "{source}: HTTP {}",
            resp.status()
        )));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ApiError::Decode(format!("{source} json: {e}")))?;
    parse(&body).ok_or_else(|| {
        ApiError::Decode(format!("{source}: could not parse price from response"))
    })
}

/// On-demand spot price proxy. Always fetches fresh from upstream — used as a
/// live ticker, not a snapshot. For the per-window frozen value, see
/// `get_window_open_price`.
pub(crate) async fn get_spot_price(
    State(s): State<Arc<AppState>>,
    Path(product): Path<String>,
    Query(q): Query<SpotPriceQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let source = q.source.unwrap_or_else(|| "kraken".to_string());
    let price = fetch_spot(&s.gamma, &product, &source).await?;
    Ok(Json(serde_json::json!({
        "product": product,
        "price": price,
        "source": source,
        "ts": chrono::Utc::now().timestamp_millis(),
    })))
}

/// Cached spot snapshot at window-open time. The first request for a given
/// `(product, interval_secs, window_start)` fetches a fresh spot tick and
/// stores it in redis with a 24h TTL. Subsequent requests (page reloads,
/// other panels) return the same frozen value — the price-to-beat hint
/// stops floating.
///
/// `interval_secs` is part of the key so 5-min and 15-min windows that
/// share an aligned `window_start` (e.g. both at `t=900*N`) get independent
/// caches and each captures spot at *its* boundary.
///
/// Concurrent first-requests are deduped via `SET NX`: whoever wins writes
/// the value, the rest read what was written.
pub(crate) async fn get_window_open_price(
    State(s): State<Arc<AppState>>,
    Path((product, interval_secs, window_start)): Path<(String, i64, i64)>,
    Query(q): Query<SpotPriceQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let source = q.source.unwrap_or_else(|| "kraken".to_string());
    let key = format!("window_open_price:{product}:{interval_secs}:{window_start}");

    // Cache hit?
    if let Some(bytes) = s.redis.get_raw(&key).await {
        if let Ok(text) = std::str::from_utf8(&bytes) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                return Ok(Json(v));
            }
        }
    }

    // Miss — fetch + persist. We use SET NX so a concurrent request can't
    // race us; the loser of the NX read-back returns the winner's value.
    let price = fetch_spot(&s.gamma, &product, &source).await?;
    let payload = serde_json::json!({
        "product": product,
        "interval_secs": interval_secs,
        "window_start": window_start,
        "price": price,
        "source": source,
        "ts": chrono::Utc::now().timestamp_millis(),
    });
    let payload_str = payload.to_string();

    // 24h TTL — long enough to survive any realistic rolling window cadence.
    let set_res: Result<bool, redis::RedisError> = redis::cmd("SET")
        .arg(&key)
        .arg(&payload_str)
        .arg("EX")
        .arg(86_400)
        .arg("NX")
        .query_async(&mut s.redis.raw())
        .await;

    match set_res {
        Ok(true) => Ok(Json(payload)),
        // Someone else cached first — return their value, not ours.
        Ok(false) => {
            if let Some(bytes) = s.redis.get_raw(&key).await {
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                        return Ok(Json(v));
                    }
                }
            }
            Ok(Json(payload))
        }
        Err(e) => Err(ApiError::Redis(format!("SET NX: {e}"))),
    }
}
