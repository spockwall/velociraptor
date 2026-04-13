use serde::{Deserialize, Serialize};

/// Messages published on `CONTROL_SOCKET` by `backend` / CLI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    Shutdown,
    Pause { service: String },
    Resume { service: String },
    StrategyParams(serde_json::Value),
    TerminateStrategy,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_roundtrip() {
        let msgs = vec![
            ControlMessage::Shutdown,
            ControlMessage::Pause { service: "executor".into() },
            ControlMessage::StrategyParams(serde_json::json!({ "k": 1 })),
            ControlMessage::TerminateStrategy,
        ];
        for m in &msgs {
            let bytes = rmp_serde::to_vec_named(m).expect("encode");
            let back: ControlMessage = rmp_serde::from_slice(&bytes).expect("decode");
            assert_eq!(m, &back);
        }
    }
}
