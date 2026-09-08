//! HTTP client used by the node agent to communicate with the control plane.
//!
//! Handles registration and periodic heartbeat reporting.

use std::time::Duration;

use serde::Serialize;

/// Registration payload sent to the control plane.
#[derive(Debug, Serialize)]
pub struct RegisterPayload {
    pub node_id: String,
    pub address: String,
    pub max_connections: u32,
    pub bandwidth_limit: u64,
}

/// Heartbeat payload sent to the control plane.
#[derive(Debug, Serialize)]
pub struct HeartbeatPayload {
    pub active_connections: u32,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub latency_ms: u32,
    pub available_bandwidth: u64,
}

/// A minimal HTTP client for the control plane API.
#[derive(Debug, Clone)]
pub struct ControlPlaneClient {
    base_url: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl ControlPlaneClient {
    pub fn new(base_url: impl Into<String>, token: Option<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token,
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(10))
                .build()
                .expect("reqwest client builds"),
        }
    }

    fn authed(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(t) => builder.bearer_auth(t),
            None => builder,
        }
    }

    /// Register this node with the control plane.
    pub async fn register(&self, payload: &RegisterPayload) -> anyhow::Result<()> {
        let resp = self
            .authed(
                self.http
                    .post(format!("{}/nodes/register", self.base_url))
                    .json(payload),
            )
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("control plane unreachable: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("register failed ({status}): {body}"));
        }
        Ok(())
    }

    /// Send a heartbeat to the control plane.
    pub async fn heartbeat(&self, node_id: &str, payload: &HeartbeatPayload) -> anyhow::Result<()> {
        let resp = self
            .authed(
                self.http
                    .post(format!("{}/nodes/{}/heartbeat", self.base_url, node_id))
                    .json(payload),
            )
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("control plane unreachable: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("heartbeat failed ({status}): {body}"));
        }
        Ok(())
    }
}
