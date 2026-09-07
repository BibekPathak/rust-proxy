//! Structured logging setup based on `tracing` and `tracing-subscriber`.

use std::str::FromStr;

use tracing_subscriber::EnvFilter;

/// The logging level to configure the global subscriber with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl FromStr for LogLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "trace" => Ok(Self::Trace),
            "debug" => Ok(Self::Debug),
            "info" => Ok(Self::Info),
            "warn" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            other => Err(format!("unknown log level: {other}")),
        }
    }
}

impl LogLevel {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// Initialize the global tracing subscriber.
///
/// By default we emit human-readable, structured output enriched with the
/// `RUST_LOG`-style filter derived from [`LogLevel`]. Setting `json` to `true`
/// switches to JSON lines, which is convenient for running under systemd or in
/// containers where the supervisor parses structured logs.
pub fn init(level: LogLevel, json: bool) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("rustproxy={}", level.as_str())));

    let base = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_thread_ids(false);

    if json {
        base.json().init();
    } else {
        base.init();
    }
}
