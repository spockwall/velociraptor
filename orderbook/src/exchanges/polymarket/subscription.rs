pub struct PolymarketSubMsgBuilder {
    asset_ids: Vec<String>,
}

impl PolymarketSubMsgBuilder {
    pub fn new() -> Self {
        Self { asset_ids: vec![] }
    }

    pub fn with_asset(mut self, asset_id: &str) -> Self {
        self.asset_ids.push(asset_id.to_string());
        self
    }

    pub fn build(self) -> String {
        serde_json::json!({
            "assets_ids": self.asset_ids,
            "type": "market"
        })
        .to_string()
    }
}

impl Default for PolymarketSubMsgBuilder {
    fn default() -> Self {
        Self::new()
    }
}
