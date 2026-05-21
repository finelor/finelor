use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{info, warn};

use crate::web::events::AppEvent;

use super::contracts::GatewayMessageResponse;
use super::gateway::AgentGatewayState;
use super::interventions::{
    InterventionTarget, build_document_intervention, claim_intervention_delivery,
    document_intervention_candidate_by_event_id, release_intervention_delivery_claim,
};

#[async_trait]
pub trait OutboundChannelAdapter: Send + Sync {
    async fn send_gateway_response(
        &self,
        target: &InterventionTarget,
        response: GatewayMessageResponse,
    ) -> anyhow::Result<()>;
}

pub struct GatewayEventDispatcher {
    state: AgentGatewayState,
    adapters: HashMap<String, Arc<dyn OutboundChannelAdapter>>,
}

impl GatewayEventDispatcher {
    pub fn new(state: AgentGatewayState) -> Self {
        Self {
            state,
            adapters: HashMap::new(),
        }
    }

    pub fn with_adapter(
        mut self,
        channel_type: impl Into<String>,
        adapter: Arc<dyn OutboundChannelAdapter>,
    ) -> Self {
        self.adapters
            .insert(channel_type.into().trim().to_uppercase(), adapter);
        self
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let mut receiver = self.state.events.subscribe();

        loop {
            match receiver.recv().await {
                Ok(event) => {
                    if let Err(err) = self.handle_event(event).await {
                        warn!(error = %err, "Gateway event dispatch failed");
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped = skipped, "Gateway event dispatcher lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    warn!("Gateway event bus closed");
                    return Ok(());
                }
            }
        }
    }

    async fn handle_event(&self, event: AppEvent) -> anyhow::Result<()> {
        if event.event_type != "document.event_recorded" {
            return Ok(());
        }

        let Some(event_id) = event
            .payload
            .get("event_id")
            .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
        else {
            warn!(app_event_id = %event.id, "Document event notification missing event_id");
            return Ok(());
        };

        let Some(candidate) =
            document_intervention_candidate_by_event_id(&self.state.pool, event_id).await?
        else {
            warn!(event_id = %event_id, "Document event notification referenced missing event");
            return Ok(());
        };

        let Some(intervention) = build_document_intervention(&self.state.pool, candidate).await?
        else {
            return Ok(());
        };

        let Some(target) = intervention.target.as_ref() else {
            return Ok(());
        };

        let channel_type = target.channel_type.trim().to_uppercase();
        let Some(adapter) = self.adapters.get(&channel_type) else {
            warn!(
                channel_type = channel_type,
                event_id = %intervention.event_id,
                "No outbound channel adapter registered"
            );
            return Ok(());
        };

        let claimed =
            claim_intervention_delivery(&self.state.ephemeral_store, &intervention).await?;
        if !claimed {
            return Ok(());
        }

        if let Err(err) = adapter
            .send_gateway_response(target, intervention.response.clone())
            .await
        {
            let _ = release_intervention_delivery_claim(&self.state.ephemeral_store, &intervention)
                .await;
            return Err(err);
        }

        info!(
            event_id = %intervention.event_id,
            workspace_id = %intervention.workspace_id,
            channel_type = channel_type,
            "Gateway intervention delivered"
        );

        Ok(())
    }
}
