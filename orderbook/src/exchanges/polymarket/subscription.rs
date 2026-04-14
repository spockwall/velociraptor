use serde_json::{Map, Value, json};

/// Builds Polymarket WebSocket subscription messages for both the public
/// market channel and the private user channel.
///
/// - **Market channel** (`type: "market"`): subscribe with asset IDs.
/// - **User channel** (`type: "user"`): call `.with_auth(...)` to embed L2
///   credentials; optionally filter by condition IDs.
///
/// Only fields that have been set are included in the final JSON — empty
/// collections and absent optional fields are omitted entirely.
///
/// # Market channel example
/// ```
/// use orderbook::PolymarketSubMsgBuilder;
/// let msg = PolymarketSubMsgBuilder::new()
///     .with_asset("7546712...")
///     .with_asset("3842963...")
///     .build();
/// // {"assets_ids":["7546712...","3842963..."],"type":"market"}
/// ```
///
/// # User channel example
/// ```
/// use orderbook::PolymarketSubMsgBuilder;
/// let msg = PolymarketSubMsgBuilder::new()
///     .with_auth("my-api-key", "my-secret", "my-passphrase")
///     .with_condition("0xbd31dc8a...")
///     .build();
/// // {"auth":{...},"markets":["0xbd31dc8a..."],"type":"user"}
/// ```

#[derive(Default)]
pub struct PolymarketSubMsgBuilder {
    // ── Market channel fields ─────────────────────────────────────────────────
    asset_ids: Vec<String>,

    // ── User channel fields ───────────────────────────────────────────────────
    auth: Option<(String, String, String)>, // (api_key, secret, passphrase)
    condition_ids: Vec<String>,
}

impl PolymarketSubMsgBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    // ── Market channel ────────────────────────────────────────────────────────

    /// Subscribe to a token asset ID (YES or NO side of a market).
    /// Repeatable — each call adds one token. Always add both YES and NO
    /// tokens together so `price_change` events arrive for both.
    pub fn with_asset(mut self, asset_id: &str) -> Self {
        self.asset_ids.push(asset_id.to_string());
        self
    }

    // ── User channel ──────────────────────────────────────────────────────────

    /// Embed L2 API credentials — switches the subscription to `type: "user"`.
    ///
    /// Credentials come from the Polymarket CLOB API (`POST /auth/api-key`).
    pub fn with_auth(mut self, api_key: &str, secret: &str, passphrase: &str) -> Self {
        self.auth = Some((
            api_key.to_string(),
            secret.to_string(),
            passphrase.to_string(),
        ));
        self
    }

    /// Filter user-channel events to a specific market by its **condition ID**
    /// (e.g. `"0xbd31dc8a..."`). Repeatable. Only relevant when `.with_auth`
    /// has been called; omit to receive events for all markets.
    pub fn with_condition(mut self, condition_id: &str) -> Self {
        self.condition_ids.push(condition_id.to_string());
        self
    }

    // ── Build ─────────────────────────────────────────────────────────────────

    /// Produce the JSON subscription string.
    ///
    /// Fields are only included when they carry a value:
    /// - `auth` — only when `.with_auth(...)` was called
    /// - `assets_ids` — only when at least one asset was added
    /// - `markets` — only when at least one condition was added
    /// - `type` — `"user"` when auth is present, otherwise `"market"`
    pub fn build(self) -> String {
        let mut obj = Map::new();

        // type — drive by whether auth was supplied
        let channel_type = if self.auth.is_some() {
            "user"
        } else {
            "market"
        };
        obj.insert("type".into(), json!(channel_type));

        // auth block — user channel only
        if let Some((key, secret, passphrase)) = self.auth {
            obj.insert(
                "auth".into(),
                json!({
                    "apiKey":     key,
                    "secret":     secret,
                    "passphrase": passphrase,
                }),
            );
        }

        // assets_ids — market channel
        if !self.asset_ids.is_empty() {
            obj.insert("assets_ids".into(), json!(self.asset_ids));
        }

        // markets — user channel condition filter
        if !self.condition_ids.is_empty() {
            obj.insert("markets".into(), json!(self.condition_ids));
        }

        Value::Object(obj).to_string()
    }
}

// Keep the old name as a type alias so existing call-sites don't break.
pub use PolymarketSubMsgBuilder as PolymarketUserSubMsgBuilder;

#[cfg(test)]
mod tests {
    use super::*;

    // ── Market channel ────────────────────────────────────────────────────────

    #[test]
    fn market_single_asset() {
        let v: serde_json::Value =
            serde_json::from_str(&PolymarketSubMsgBuilder::new().with_asset("abc123").build())
                .unwrap();
        assert_eq!(v["type"], "market");
        assert_eq!(v["assets_ids"][0], "abc123");
        assert!(v.get("auth").is_none());
        assert!(v.get("markets").is_none());
    }

    #[test]
    fn market_multiple_assets() {
        let v: serde_json::Value = serde_json::from_str(
            &PolymarketSubMsgBuilder::new()
                .with_asset("yes-token")
                .with_asset("no-token")
                .build(),
        )
        .unwrap();
        assert_eq!(v["assets_ids"].as_array().unwrap().len(), 2);
        assert!(v.get("auth").is_none());
    }

    #[test]
    fn market_no_assets_omits_field() {
        let v: serde_json::Value =
            serde_json::from_str(&PolymarketSubMsgBuilder::new().build()).unwrap();
        assert_eq!(v["type"], "market");
        assert!(v.get("assets_ids").is_none());
    }

    // ── User channel ──────────────────────────────────────────────────────────

    #[test]
    fn user_embeds_auth_switches_type() {
        let v: serde_json::Value = serde_json::from_str(
            &PolymarketSubMsgBuilder::new()
                .with_auth("my-key", "my-secret", "my-pass")
                .with_condition("0xabc123")
                .build(),
        )
        .unwrap();
        assert_eq!(v["type"], "user");
        assert_eq!(v["auth"]["apiKey"], "my-key");
        assert_eq!(v["auth"]["secret"], "my-secret");
        assert_eq!(v["auth"]["passphrase"], "my-pass");
        assert_eq!(v["markets"][0], "0xabc123");
        assert!(v.get("assets_ids").is_none()); // no assets added
    }

    #[test]
    fn user_no_conditions_omits_markets_field() {
        let v: serde_json::Value = serde_json::from_str(
            &PolymarketSubMsgBuilder::new()
                .with_auth("k", "s", "p")
                .build(),
        )
        .unwrap();
        assert_eq!(v["type"], "user");
        assert!(v.get("markets").is_none()); // no conditions → field absent
    }

    #[test]
    fn user_multiple_conditions() {
        let v: serde_json::Value = serde_json::from_str(
            &PolymarketSubMsgBuilder::new()
                .with_auth("k", "s", "p")
                .with_condition("0xaaa")
                .with_condition("0xbbb")
                .build(),
        )
        .unwrap();
        assert_eq!(v["markets"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn combined_assets_and_auth_both_included() {
        // Edge case: caller adds both assets and auth — both fields appear.
        let v: serde_json::Value = serde_json::from_str(
            &PolymarketSubMsgBuilder::new()
                .with_asset("tok1")
                .with_auth("k", "s", "p")
                .build(),
        )
        .unwrap();
        assert_eq!(v["type"], "user"); // auth wins
        assert_eq!(v["assets_ids"][0], "tok1");
        assert!(v["auth"]["apiKey"] == "k");
    }
}
