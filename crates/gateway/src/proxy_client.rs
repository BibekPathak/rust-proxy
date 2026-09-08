//! HTTP client used by the gateway to query the control plane.
//!
//! Keeps the gateway decoupled from the control-plane crate: it only speaks
//! the documented JSON HTTP API and copies the fields it needs as plain
//! values.

use serde::Deserialize;

use crate::error::{GatewayError, Result};

/// The subset of control-plane node data the gateway needs.
#[derive(Debug, Clone)]
pub struct RemoteNode {
    pub id: String,
    pub forward_address: String,
}

/// Wire shape of the control plane's `/nodes/select` response.
#[derive(Debug, Deserialize)]
struct SelectResponse {
    id: String,
    address: String,
}

/// A minimal JSON client for a subset of the control plane API.
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
                .connect_timeout(std::time::Duration::from_secs(5))
                .timeout(std::time::Duration::from_secs(10))
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

    /// Ask the control plane for the best eligible node.
    pub async fn select_node(&self) -> Result<RemoteNode> {
        let resp = self
            .authed(self.http.post(format!("{}/nodes/select", self.base_url)))
            .send()
            .await
            .map_err(|e| GatewayError::Relay(format!("control plane unreachable: {e}")))?;

        if resp.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE {
            return Err(GatewayError::NoNode);
        }
        if !resp.status().is_success() {
            return Err(GatewayError::Relay(format!(
                "control plane select failed with status {}",
                resp.status()
            )));
        }

        let node: SelectResponse = resp
            .json()
            .await
            .map_err(|e| GatewayError::Relay(format!("invalid select response: {e}")))?;
        Ok(RemoteNode {
            id: node.id,
            forward_address: node.address,
        })
    }
}
