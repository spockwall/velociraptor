pub mod binance;
pub mod hyperliquid;
pub mod kalshi;
pub mod okx;
pub mod polymarket;

pub use binance::BinanceCredentials;
pub use hyperliquid::HyperliquidCredentials;
pub use kalshi::KalshiCredentials;
pub use okx::OkxCredentials;
pub use polymarket::PolymarketCredentials;

/// Read a credentials YAML file, extract one top-level key, deserialize it
/// into `T`, and exit the process on any failure.
pub(crate) fn load_section_or_exit<T, P>(path: P, section: &str) -> T
where
    T: for<'de> serde::Deserialize<'de>,
    P: AsRef<std::path::Path>,
{
    let path = path.as_ref();
    let contents = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("failed to read '{}': {e}", path.display());
        std::process::exit(1);
    });
    let mut root: serde_yaml::Mapping =
        serde_yaml::from_str(&contents).unwrap_or_else(|e| {
            eprintln!("failed to parse '{}': {e}", path.display());
            std::process::exit(1);
        });
    let value = root
        .remove(section)
        .unwrap_or_else(|| {
            eprintln!("missing '{section}' key in '{}'", path.display());
            std::process::exit(1);
        });
    serde_yaml::from_value(value).unwrap_or_else(|e| {
        eprintln!("failed to parse '{section}' in '{}': {e}", path.display());
        std::process::exit(1);
    })
}

pub(crate) fn exit_if_empty(field: &str, exchange: &str, value: &str) {
    if value.is_empty() {
        eprintln!("{exchange} credentials: {field} is empty");
        std::process::exit(1);
    }
}
