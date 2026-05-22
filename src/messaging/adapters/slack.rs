use std::future::pending;
use std::sync::Arc;

use serde_json::{Value, json};
use slack_morphism::prelude::*;
use tracing::{error, info, warn};

use crate::db;
use crate::ingestion::{self, DocumentArtifactInput, IngestionInput};
use crate::messaging::contracts::{
    ActionButton, AgentInboundMessage, GatewayAttachment, GatewayMessageFormat,
    GatewayMessageResponse, MessageSource,
};
use crate::messaging::dispatch::OutboundChannelAdapter;
use crate::messaging::gateway::{AgentGatewayState, encode_document_action, parse_document_action};
use crate::messaging::interventions::InterventionTarget;

#[derive(Clone)]
pub struct SlackOutboundAdapter {
    client: SlackHttpClient,
}

impl SlackOutboundAdapter {
    pub fn new(bot_token: impl Into<String>) -> Self {
        Self {
            client: SlackHttpClient::new(bot_token),
        }
    }
}

#[async_trait::async_trait]
impl OutboundChannelAdapter for SlackOutboundAdapter {
    async fn send_gateway_response(
        &self,
        target: &InterventionTarget,
        response: GatewayMessageResponse,
    ) -> anyhow::Result<()> {
        let thread_ts = target
            .metadata
            .get("thread_ts")
            .and_then(Value::as_str)
            .or_else(|| target.metadata.get("message_ts").and_then(Value::as_str));
        self.client
            .send_gateway_response(&target.channel_identifier, thread_ts, response)
            .await
    }
}

pub async fn run_slack_socket_mode(state: AgentGatewayState) -> anyhow::Result<()> {
    let app_token = SlackApiToken::new(SlackApiTokenValue(
        state.config.messaging.slack.app_token.clone(),
    ));
    let connector = SlackClientHyperConnector::new()?;
    let slack_client = Arc::new(SlackClient::new(connector));
    let environment =
        Arc::new(SlackClientEventsListenerEnvironment::new(slack_client).with_user_state(state));
    let callbacks = SlackSocketModeListenerCallbacks::new()
        .with_command_events(slack_command_callback)
        .with_push_events(slack_push_callback)
        .with_interaction_events(slack_interaction_callback);
    let listener = SlackClientSocketModeListener::new(
        &SlackClientSocketModeConfig::new(),
        environment,
        callbacks,
    );

    listener.listen_for(&app_token).await?;
    listener.start().await;
    info!("Slack Socket Mode listener started");
    pending::<()>().await;
    #[allow(unreachable_code)]
    Ok(())
}

async fn slack_command_callback<SCHC>(
    event: SlackCommandEvent,
    _client: Arc<SlackClient<SCHC>>,
    states: SlackClientEventsUserState,
) -> UserCallbackResult<SlackCommandEventResponse>
where
    SCHC: SlackClientHttpConnector + Send + Sync + 'static,
{
    let state = slack_state_from_listener(&states).await?;
    handle_slack_command(event, state).await
}

async fn slack_push_callback<SCHC>(
    event: SlackPushEventCallback,
    _client: Arc<SlackClient<SCHC>>,
    states: SlackClientEventsUserState,
) -> UserCallbackResult<()>
where
    SCHC: SlackClientHttpConnector + Send + Sync + 'static,
{
    let state = slack_state_from_listener(&states).await?;
    if !claim_slack_event(&state, &event.event_id.0).await {
        info!(event_id = %event.event_id.0, "Skipping duplicate Slack push event");
        return Ok(());
    }
    tokio::spawn(async move {
        handle_slack_push_event(event, state).await;
    });
    Ok(())
}

async fn slack_interaction_callback<SCHC>(
    event: SlackInteractionEvent,
    _client: Arc<SlackClient<SCHC>>,
    states: SlackClientEventsUserState,
) -> UserCallbackResult<()>
where
    SCHC: SlackClientHttpConnector + Send + Sync + 'static,
{
    let state = slack_state_from_listener(&states).await?;
    if let Some(key) = slack_interaction_dedupe_key(&event)
        && !claim_slack_event(&state, &key).await
    {
        info!(event_key = %key, "Skipping duplicate Slack interaction event");
        return Ok(());
    }
    tokio::spawn(async move {
        handle_slack_interaction(event, state).await;
    });
    Ok(())
}

async fn slack_state_from_listener(
    states: &SlackClientEventsUserState,
) -> UserCallbackResult<AgentGatewayState> {
    let guard = states.read().await;
    guard
        .get_user_state::<AgentGatewayState>()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("missing Slack listener gateway state").into())
}

async fn handle_slack_command(
    event: SlackCommandEvent,
    state: AgentGatewayState,
) -> UserCallbackResult<SlackCommandEventResponse> {
    let text = normalize_slack_command_text(event.command.0.as_str(), event.text.as_deref());
    let source = slack_source(
        event.team_id.0.as_str(),
        event.channel_id.0.as_str(),
        Some(event.user_id.0.as_str()),
        None,
        None,
        json!({
            "entrypoint": "slash_command",
            "command": event.command.0,
            "channel_name": event.channel_name,
        }),
    );
    if !slack_channel_allowed(&state, &source.channel_identifier) {
        return Ok(SlackCommandEventResponse {
            content: SlackMessageContent::new()
                .with_text("Finelor is not enabled for this Slack channel.".to_string()),
            response_type: Some(SlackMessageResponseType::Ephemeral),
        });
    }
    let response_url = event.response_url.0.to_string();
    tokio::spawn(async move {
        handle_slack_command_async(source, text, response_url, state).await;
    });

    Ok(empty_slack_command_ack())
}

fn empty_slack_command_ack() -> SlackCommandEventResponse {
    // Slack treats an empty 200 response as acknowledgement only. Keep this
    // silent; the actual slash-command result is posted later via response_url.
    SlackCommandEventResponse {
        content: SlackMessageContent::new(),
        response_type: None,
    }
}

async fn handle_slack_command_async(
    source: MessageSource,
    text: String,
    response_url: String,
    state: AgentGatewayState,
) {
    ensure_slack_channel_identity(&state, &source).await;
    let channel_id = source.channel_identifier.clone();
    let client = SlackHttpClient::new(state.config.messaging.slack.bot_token.clone());
    match state
        .handle_inbound_message(AgentInboundMessage::Text { source, text })
        .await
    {
        Ok(response) => {
            if let Err(err) = client
                .send_response_url_gateway_response(&response_url, &channel_id, response)
                .await
            {
                error!(error = %err, channel_id = %channel_id, "Failed to send Slack slash command response");
            }
        }
        Err(err) => {
            error!(error = %err, channel_id = %channel_id, "Slack slash command failed");
            let _ = client
                .post_response_url_message(
                    &response_url,
                    "I could not answer that right now. Please try again in a moment.",
                    GatewayMessageFormat::PlainText,
                    None,
                )
                .await;
        }
    }
}

async fn handle_slack_push_event(event: SlackPushEventCallback, state: AgentGatewayState) {
    match event.event {
        SlackEventCallbackBody::AppMention(mention) => {
            let text = mention
                .content
                .text
                .as_deref()
                .map(strip_slack_mentions)
                .unwrap_or_default();
            let source = slack_source(
                event.team_id.0.as_str(),
                mention.channel.0.as_str(),
                Some(mention.user.0.as_str()),
                Some(mention.origin.ts.0.as_str()),
                Some(
                    mention
                        .origin
                        .thread_ts
                        .as_ref()
                        .map(|ts| ts.0.as_str())
                        .unwrap_or(mention.origin.ts.0.as_str()),
                ),
                json!({"entrypoint": "app_mention"}),
            );
            handle_slack_text(source, text, state).await;
        }
        SlackEventCallbackBody::Message(message) => {
            if message.hidden == Some(true)
                || message.subtype == Some(SlackMessageEventType::BotMessage)
                || message.sender.bot_id.is_some()
            {
                return;
            }

            let Some(channel) = message.origin.channel.as_ref() else {
                return;
            };
            let is_dm = message
                .origin
                .channel_type
                .as_ref()
                .map(|channel_type| channel_type.0.eq_ignore_ascii_case("im"))
                .unwrap_or(false);
            let Some(content) = message.content.as_ref() else {
                return;
            };
            let source = slack_source(
                event.team_id.0.as_str(),
                channel.0.as_str(),
                message.sender.user.as_ref().map(|user| user.0.as_str()),
                Some(message.origin.ts.0.as_str()),
                message.origin.thread_ts.as_ref().map(|ts| ts.0.as_str()),
                json!({
                    "entrypoint": "message",
                    "channel_type": message.origin.channel_type.as_ref().map(|ct| ct.0.as_str()),
                }),
            );

            if let Some(files) = content.files.as_ref() {
                if !is_dm && !slack_text_addresses_app(content.text.as_deref()) {
                    return;
                }
                for file in files {
                    handle_slack_file(source.clone(), file, state.clone()).await;
                }
                return;
            }

            if !is_dm {
                return;
            }

            let text = content.text.as_deref().unwrap_or_default().trim();
            if !text.is_empty() {
                handle_slack_text(source, text.to_string(), state).await;
            }
        }
        _ => {}
    }
}

async fn handle_slack_text(source: MessageSource, text: String, state: AgentGatewayState) {
    if !slack_channel_allowed(&state, &source.channel_identifier) {
        return;
    }
    ensure_slack_channel_identity(&state, &source).await;
    let channel_id = source.channel_identifier.clone();
    let thread_ts = source
        .metadata
        .get("thread_ts")
        .and_then(Value::as_str)
        .map(str::to_string);
    let client = SlackHttpClient::new(state.config.messaging.slack.bot_token.clone());
    match state
        .handle_inbound_message(AgentInboundMessage::Text { source, text })
        .await
    {
        Ok(response) => {
            if let Err(err) = client
                .send_gateway_response(&channel_id, thread_ts.as_deref(), response)
                .await
            {
                error!(error = %err, channel_id = %channel_id, "Failed to send Slack response");
            }
        }
        Err(err) => {
            error!(error = %err, channel_id = %channel_id, "Slack text handling failed");
            let _ = client
                .post_message(
                    &channel_id,
                    thread_ts.as_deref(),
                    "I could not answer that right now. Please try again in a moment.",
                    GatewayMessageFormat::PlainText,
                    None,
                )
                .await;
        }
    }
}

async fn handle_slack_file(source: MessageSource, file: &SlackFile, state: AgentGatewayState) {
    if !slack_channel_allowed(&state, &source.channel_identifier) {
        return;
    }
    ensure_slack_channel_identity(&state, &source).await;
    let client = SlackHttpClient::new(state.config.messaging.slack.bot_token.clone());
    let Some(url) = file
        .url_private_download
        .as_ref()
        .or(file.url_private.as_ref())
    else {
        warn!(file_id = %file.id.0, "Slack file has no downloadable URL");
        return;
    };

    let bytes = match client.download_private_file(url.as_str()).await {
        Ok(bytes) => bytes,
        Err(err) => {
            error!(error = %err, file_id = %file.id.0, "Failed to download Slack file");
            return;
        }
    };
    let filename = file
        .name
        .clone()
        .or_else(|| file.title.clone())
        .unwrap_or_else(|| format!("slack-file-{}", file.id.0));
    let mime_type = file
        .mimetype
        .as_ref()
        .map(|mime| mime.0.clone())
        .unwrap_or_else(|| "application/octet-stream".to_string());

    let result = ingestion::ingest_document(
        &state.pool,
        &state.config,
        &state.queue_producer,
        Some(&state.events),
        IngestionInput {
            file_bytes: bytes,
            filename: filename.clone(),
            mime_type: mime_type.clone(),
            document_type: "receipt".to_string(),
            artifact: DocumentArtifactInput {
                channel_type: db::ChannelType::Slack.as_str().to_string(),
                channel_identifier: source.channel_identifier.clone(),
                profile_identifier: source.profile_identifier.clone(),
                external_artifact_id: Some(file.id.0.clone()),
                source_timestamp: source.source_timestamp.clone(),
                original_filename: Some(filename),
                metadata: source.metadata.clone(),
            },
        },
    )
    .await;

    let message = match result {
        Ok(saved) => format!("Received {}. I will process it now.", saved.short_ref),
        Err(err) => {
            error!(error = %err, file_id = %file.id.0, "Slack file ingestion failed");
            "I could not ingest that file. Please try again in a moment.".to_string()
        }
    };
    let _ = client
        .post_message(
            &source.channel_identifier,
            source.metadata.get("thread_ts").and_then(Value::as_str),
            &message,
            GatewayMessageFormat::PlainText,
            None,
        )
        .await;
}

async fn handle_slack_interaction(event: SlackInteractionEvent, state: AgentGatewayState) {
    let SlackInteractionEvent::BlockActions(event) = event else {
        return;
    };
    let Some(action_value) = event
        .actions
        .as_ref()
        .and_then(|actions| actions.first())
        .and_then(|action| action.value.as_deref())
    else {
        return;
    };
    let action = match parse_document_action(action_value) {
        Ok(action) => action,
        Err(err) => {
            error!(error = %err, action = action_value, "Failed to parse Slack action");
            return;
        }
    };
    let Some(channel) = event.channel.as_ref() else {
        return;
    };
    let user_id = event.user.as_ref().map(|user| user.id.0.as_str());
    let message_ts = match &event.container {
        SlackInteractionActionContainer::Message(container) => {
            Some(container.message_ts.0.as_str())
        }
        SlackInteractionActionContainer::MessageAttachment(container) => {
            Some(container.message_ts.0.as_str())
        }
        SlackInteractionActionContainer::View(_) => None,
    };
    let source = slack_source(
        event.team.id.0.as_str(),
        channel.id.0.as_str(),
        user_id,
        message_ts,
        message_ts,
        json!({"entrypoint": "block_action"}),
    );
    if !slack_channel_allowed(&state, &source.channel_identifier) {
        return;
    }
    ensure_slack_channel_identity(&state, &source).await;
    let client = SlackHttpClient::new(state.config.messaging.slack.bot_token.clone());
    match state.handle_document_action(&source, action).await {
        Ok(response) => {
            let message = GatewayMessageResponse {
                message: response.message,
                format: response.format,
                attachments: response.attachments,
                buttons: response.buttons,
            };
            if let Err(err) = client
                .send_gateway_response(
                    &source.channel_identifier,
                    source.metadata.get("thread_ts").and_then(Value::as_str),
                    message,
                )
                .await
            {
                error!(error = %err, "Failed to send Slack action response");
            }
        }
        Err(err) => {
            error!(error = %err, "Slack action handling failed");
            let _ = client
                .post_message(
                    &source.channel_identifier,
                    source.metadata.get("thread_ts").and_then(Value::as_str),
                    "Failed to process that action.",
                    GatewayMessageFormat::PlainText,
                    None,
                )
                .await;
        }
    }
}

fn slack_source(
    team_id: &str,
    channel_id: &str,
    user_id: Option<&str>,
    message_ts: Option<&str>,
    thread_ts: Option<&str>,
    extra_metadata: Value,
) -> MessageSource {
    let mut metadata = json!({
        "team_id": team_id,
        "message_ts": message_ts,
        "thread_ts": thread_ts,
    });
    merge_json_object(&mut metadata, extra_metadata);
    MessageSource {
        channel: db::ChannelType::Slack,
        channel_identifier: channel_id.to_string(),
        profile_identifier: user_id.map(str::to_string),
        message_id: message_ts.map(str::to_string),
        source_timestamp: message_ts.map(str::to_string),
        metadata,
    }
}

fn slack_channel_allowed(state: &AgentGatewayState, channel_id: &str) -> bool {
    state.config.messaging.slack.allowed_channel_ids.is_empty()
        || state
            .config
            .messaging
            .slack
            .allowed_channel_ids
            .iter()
            .any(|allowed| allowed == channel_id)
}

async fn ensure_slack_channel_identity(state: &AgentGatewayState, source: &MessageSource) {
    let metadata = json!({
        "team_id": source.metadata.get("team_id").and_then(Value::as_str),
        "last_user_id": source.profile_identifier,
        "last_message_ts": source.message_id,
    });
    if let Err(err) = sqlx::query(
        r#"
        UPDATE channel_identities
        SET metadata = $3, active = TRUE, updated_at = CURRENT_TIMESTAMP
        WHERE channel_type = $1 AND channel_identifier = $2
        "#,
    )
    .bind(db::ChannelType::Slack.as_str())
    .bind(&source.channel_identifier)
    .bind(&metadata)
    .execute(&state.pool)
    .await
    {
        warn!(error = %err, "Failed to update Slack channel identity");
        return;
    }

    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM channel_identities WHERE channel_type = $1 AND channel_identifier = $2)",
    )
    .bind(db::ChannelType::Slack.as_str())
    .bind(&source.channel_identifier)
    .fetch_one(&state.pool)
    .await;
    if matches!(exists, Ok(true)) {
        return;
    }

    if let Err(err) = db::insert_channel_identity(
        &state.pool,
        db::ChannelType::Slack.as_str(),
        &source.channel_identifier,
        Some(metadata),
    )
    .await
    {
        warn!(error = %err, "Failed to insert Slack channel identity");
    }
}

#[derive(Clone)]
struct SlackHttpClient {
    bot_token: String,
    http: reqwest::Client,
}

impl SlackHttpClient {
    fn new(bot_token: impl Into<String>) -> Self {
        Self {
            bot_token: bot_token.into(),
            http: reqwest::Client::new(),
        }
    }

    async fn send_gateway_response(
        &self,
        channel_id: &str,
        thread_ts: Option<&str>,
        response: GatewayMessageResponse,
    ) -> anyhow::Result<()> {
        for attachment in &response.attachments {
            if let GatewayAttachment::StoredMedia { channel_type, .. } = attachment {
                warn!(
                    channel_type = channel_type,
                    "Skipping stored media replay for Slack response"
                );
            }
        }

        if !response.message.trim().is_empty() {
            self.post_message(
                channel_id,
                thread_ts,
                &response.message,
                response.format,
                response.buttons.as_deref(),
            )
            .await?;
        }

        for attachment in response.attachments {
            if let GatewayAttachment::LocalFile { path } = attachment {
                let filename = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("finelor-export.zip")
                    .to_string();
                let bytes = tokio::fs::read(&path).await?;
                self.upload_file(channel_id, thread_ts, &filename, bytes)
                    .await?;
            }
        }

        Ok(())
    }

    async fn send_response_url_gateway_response(
        &self,
        response_url: &str,
        channel_id: &str,
        response: GatewayMessageResponse,
    ) -> anyhow::Result<()> {
        for attachment in &response.attachments {
            if let GatewayAttachment::StoredMedia { channel_type, .. } = attachment {
                warn!(
                    channel_type = channel_type,
                    "Skipping stored media replay for Slack response URL response"
                );
            }
        }

        if !response.message.trim().is_empty() {
            self.post_response_url_message(
                response_url,
                &response.message,
                response.format,
                response.buttons.as_deref(),
            )
            .await?;
        }

        for attachment in response.attachments {
            if let GatewayAttachment::LocalFile { path } = attachment {
                let filename = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("finelor-export.zip")
                    .to_string();
                let bytes = tokio::fs::read(&path).await?;
                self.upload_file(channel_id, None, &filename, bytes).await?;
            }
        }

        Ok(())
    }

    async fn post_message(
        &self,
        channel_id: &str,
        thread_ts: Option<&str>,
        text: &str,
        format: GatewayMessageFormat,
        buttons: Option<&[Vec<ActionButton>]>,
    ) -> anyhow::Result<()> {
        let mut body = json!({
            "channel": channel_id,
            "text": render_slack_text(text, format),
        });
        if let Some(thread_ts) = thread_ts.filter(|value| !value.trim().is_empty()) {
            body["thread_ts"] = json!(thread_ts);
        }
        let blocks = slack_blocks(text, format, buttons);
        if !blocks.is_empty() {
            body["blocks"] = json!(blocks);
        }

        let response = self
            .http
            .post("https://slack.com/api/chat.postMessage")
            .bearer_auth(&self.bot_token)
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let payload = response.json::<Value>().await.unwrap_or_else(|_| json!({}));
        if !status.is_success() || payload.get("ok").and_then(Value::as_bool) != Some(true) {
            anyhow::bail!("Slack chat.postMessage failed: status={status}, response={payload}");
        }

        Ok(())
    }

    async fn post_response_url_message(
        &self,
        response_url: &str,
        text: &str,
        format: GatewayMessageFormat,
        buttons: Option<&[Vec<ActionButton>]>,
    ) -> anyhow::Result<()> {
        let body = slack_response_url_body(text, format, buttons);
        let response = self.http.post(response_url).json(&body).send().await?;
        let status = response.status();
        if !status.is_success() {
            let payload = response.text().await.unwrap_or_default();
            anyhow::bail!("Slack response_url post failed: status={status}, response={payload}");
        }

        Ok(())
    }

    async fn download_private_file(&self, url: &str) -> anyhow::Result<Vec<u8>> {
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.bot_token)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("Slack file download returned {status}");
        }
        Ok(response.bytes().await?.to_vec())
    }

    async fn upload_file(
        &self,
        channel_id: &str,
        thread_ts: Option<&str>,
        filename: &str,
        bytes: Vec<u8>,
    ) -> anyhow::Result<()> {
        let upload_url_response = self
            .http
            .post("https://slack.com/api/files.getUploadURLExternal")
            .bearer_auth(&self.bot_token)
            .form(&[
                ("filename", filename.to_string()),
                ("length", bytes.len().to_string()),
            ])
            .send()
            .await?;
        let status = upload_url_response.status();
        let upload_payload = upload_url_response.json::<Value>().await?;
        if !status.is_success() || upload_payload.get("ok").and_then(Value::as_bool) != Some(true) {
            anyhow::bail!(
                "Slack files.getUploadURLExternal failed: status={status}, response={upload_payload}"
            );
        }

        let upload_url = upload_payload
            .get("upload_url")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Slack upload URL response missing upload_url"))?;
        let file_id = upload_payload
            .get("file_id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Slack upload URL response missing file_id"))?;

        let upload_response = self
            .http
            .post(upload_url)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(bytes)
            .send()
            .await?;
        if !upload_response.status().is_success() {
            anyhow::bail!(
                "Slack external file upload returned {}",
                upload_response.status()
            );
        }

        let mut complete_body = json!({
            "files": [{"id": file_id, "title": filename}],
            "channel_id": channel_id,
        });
        if let Some(thread_ts) = thread_ts.filter(|value| !value.trim().is_empty()) {
            complete_body["thread_ts"] = json!(thread_ts);
        }
        let complete_response = self
            .http
            .post("https://slack.com/api/files.completeUploadExternal")
            .bearer_auth(&self.bot_token)
            .json(&complete_body)
            .send()
            .await?;
        let status = complete_response.status();
        let complete_payload = complete_response
            .json::<Value>()
            .await
            .unwrap_or_else(|_| json!({}));
        if !status.is_success() || complete_payload.get("ok").and_then(Value::as_bool) != Some(true)
        {
            anyhow::bail!(
                "Slack files.completeUploadExternal failed: status={status}, response={complete_payload}"
            );
        }

        Ok(())
    }
}

fn slack_blocks(
    text: &str,
    format: GatewayMessageFormat,
    buttons: Option<&[Vec<ActionButton>]>,
) -> Vec<Value> {
    let mut blocks = vec![json!({
        "type": "section",
        "text": {
            "type": "mrkdwn",
            "text": render_slack_text(text, format),
        }
    })];

    if let Some(button_rows) = buttons {
        for (row_index, row) in button_rows.iter().enumerate() {
            let elements: Vec<Value> = row
                .iter()
                .enumerate()
                .map(|(button_index, button)| {
                    json!({
                        "type": "button",
                        "text": {
                            "type": "plain_text",
                            "text": button.label,
                        },
                        "action_id": format!("finelor_document_action_{row_index}_{button_index}"),
                        "value": encode_document_action(button.action.clone()),
                    })
                })
                .collect();
            if !elements.is_empty() {
                blocks.push(json!({
                    "type": "actions",
                    "block_id": format!("finelor_actions_{row_index}"),
                    "elements": elements,
                }));
            }
        }
    }

    blocks
}

fn slack_response_url_body(
    text: &str,
    format: GatewayMessageFormat,
    buttons: Option<&[Vec<ActionButton>]>,
) -> Value {
    json!({
        "response_type": "in_channel",
        "text": render_slack_text(text, format),
        "blocks": slack_blocks(text, format, buttons),
    })
}

fn render_slack_text(text: &str, format: GatewayMessageFormat) -> String {
    match format {
        GatewayMessageFormat::PlainText => escape_slack_text(text),
        GatewayMessageFormat::Markdown => render_slack_markdown(text),
    }
}

fn render_slack_markdown(text: &str) -> String {
    let mut output = String::new();
    let mut index = 0;

    while index < text.len() {
        let rest = &text[index..];
        if rest.starts_with("**") {
            output.push('*');
            index += 2;
            continue;
        }

        if rest.starts_with('[')
            && let Some(close_label) = rest.find("](")
            && let Some(close_url) = rest[close_label + 2..].find(')')
        {
            let label = &rest[1..close_label];
            let url_start = close_label + 2;
            let url_end = url_start + close_url;
            let url = &rest[url_start..url_end];
            output.push('<');
            output.push_str(&escape_slack_link_url(url));
            output.push('|');
            output.push_str(&render_slack_markdown(label));
            output.push('>');
            index += url_end + 1;
            continue;
        }

        let Some(ch) = rest.chars().next() else {
            break;
        };
        push_escaped_slack_char(&mut output, ch);
        index += ch.len_utf8();
    }

    output
}

fn escape_slack_text(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for ch in text.chars() {
        push_escaped_slack_char(&mut output, ch);
    }
    output
}

fn push_escaped_slack_char(output: &mut String, ch: char) {
    match ch {
        '&' => output.push_str("&amp;"),
        '<' => output.push_str("&lt;"),
        '>' => output.push_str("&gt;"),
        _ => output.push(ch),
    }
}

fn escape_slack_link_url(url: &str) -> String {
    url.replace('|', "%7C").replace('>', "%3E")
}

fn normalize_slack_command_text(command: &str, text: Option<&str>) -> String {
    let command_name = command
        .trim_start_matches('/')
        .split('@')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let Some(gateway_command) = slack_gateway_command_name(&command_name) else {
        return "/help".to_string();
    };

    let trimmed = text.map(str::trim).filter(|value| !value.is_empty());
    let Some(trimmed) = trimmed else {
        return format!("/{gateway_command}");
    };
    if trimmed.starts_with('/') {
        return trimmed.to_string();
    }

    format!("/{gateway_command} {trimmed}")
}

fn slack_gateway_command_name(word: &str) -> Option<&'static str> {
    Some(match word {
        "help" => "help",
        "documents" => "documents",
        "pending" => "pending",
        "ready" => "ready",
        "last" => "last",
        "why" => "why",
        "review" => "review",
        "retry" => "retry",
        "export" => "export",
        "overview" => "status",
        _ => return None,
    })
}

fn slack_text_addresses_app(text: Option<&str>) -> bool {
    text.is_some_and(|text| text.split_whitespace().any(is_slack_user_mention))
}

fn is_slack_user_mention(part: &str) -> bool {
    part.starts_with("<@") && part.ends_with('>')
}

async fn claim_slack_event(state: &AgentGatewayState, key: &str) -> bool {
    match state
        .ephemeral_store
        .claim(&format!("slack:event:{key}"), "processing", 5 * 60)
        .await
    {
        Ok(claimed) => claimed,
        Err(err) => {
            warn!(error = %err, key = %key, "Failed to claim Slack event; processing without dedupe");
            true
        }
    }
}

fn slack_interaction_dedupe_key(event: &SlackInteractionEvent) -> Option<String> {
    let SlackInteractionEvent::BlockActions(event) = event else {
        return None;
    };
    let action_value = event
        .actions
        .as_ref()
        .and_then(|actions| actions.first())
        .and_then(|action| action.value.as_deref())?;
    let channel_id = event
        .channel
        .as_ref()
        .map(|channel| channel.id.0.as_str())?;
    let user_id = event.user.as_ref().map(|user| user.id.0.as_str())?;
    let message_ts = match &event.container {
        SlackInteractionActionContainer::Message(container) => container.message_ts.0.as_str(),
        SlackInteractionActionContainer::MessageAttachment(container) => {
            container.message_ts.0.as_str()
        }
        SlackInteractionActionContainer::View(_) => return None,
    };
    Some(format!(
        "interaction:{}:{channel_id}:{message_ts}:{user_id}:{action_value}",
        event.team.id.0
    ))
}

fn strip_slack_mentions(text: &str) -> String {
    text.split_whitespace()
        .filter(|part| !(part.starts_with("<@") && part.ends_with('>')))
        .collect::<Vec<_>>()
        .join(" ")
}

fn merge_json_object(target: &mut Value, source: Value) {
    let (Some(target), Some(source)) = (target.as_object_mut(), source.as_object()) else {
        return;
    };
    for (key, value) in source {
        target.insert(key.clone(), value.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::contracts::DocumentAction;

    #[test]
    fn slack_markdown_converts_common_finelor_markdown() {
        let rendered = render_slack_text(
            "**D000001** is ready. See [export](https://example.com/a?b=1&c=2).",
            GatewayMessageFormat::Markdown,
        );

        assert_eq!(
            rendered,
            "*D000001* is ready. See <https://example.com/a?b=1&c=2|export>."
        );
    }

    #[test]
    fn slack_text_escapes_control_characters() {
        let rendered = render_slack_text("A < B & C > D", GatewayMessageFormat::PlainText);

        assert_eq!(rendered, "A &lt; B &amp; C &gt; D");
    }

    #[test]
    fn slack_blocks_use_unique_action_ids() {
        let buttons = vec![vec![
            ActionButton {
                label: "Approve".to_string(),
                action: DocumentAction::Approve { document_id: 7 },
            },
            ActionButton {
                label: "Reject".to_string(),
                action: DocumentAction::Reject { document_id: 7 },
            },
        ]];

        let blocks = slack_blocks(
            "Review **D000007**",
            GatewayMessageFormat::Markdown,
            Some(&buttons),
        );
        let elements = blocks[1]["elements"].as_array().unwrap();

        assert_eq!(blocks[1]["block_id"], "finelor_actions_0");
        assert_eq!(elements[0]["action_id"], "finelor_document_action_0_0");
        assert_eq!(elements[1]["action_id"], "finelor_document_action_0_1");
        assert_ne!(elements[0]["action_id"], elements[1]["action_id"]);
        assert_eq!(elements[0]["value"], "approve:7");
        assert_eq!(elements[1]["value"], "reject:7");
    }

    #[test]
    fn slack_response_url_body_uses_rendering_and_buttons() {
        let buttons = vec![vec![ActionButton {
            label: "Approve".to_string(),
            action: DocumentAction::Approve { document_id: 7 },
        }]];

        let body = slack_response_url_body(
            "Review **D000007**",
            GatewayMessageFormat::Markdown,
            Some(&buttons),
        );

        assert_eq!(body["response_type"], "in_channel");
        assert_eq!(body["text"], "Review *D000007*");
        assert_eq!(body["blocks"][0]["text"]["text"], "Review *D000007*");
        assert_eq!(
            body["blocks"][1]["elements"][0]["action_id"],
            "finelor_document_action_0_0"
        );
        assert_eq!(body["blocks"][1]["elements"][0]["value"], "approve:7");
    }

    #[test]
    fn empty_command_ack_serializes_without_visible_message() {
        let ack = empty_slack_command_ack();
        let value = serde_json::to_value(ack).unwrap();

        assert_eq!(value, json!({}));
    }

    #[test]
    fn slack_command_text_matches_gateway_slash_commands() {
        assert_eq!(normalize_slack_command_text("/help", None), "/help");
        assert_eq!(normalize_slack_command_text("/overview", None), "/status");
        assert_eq!(
            normalize_slack_command_text("/review", Some("D000123")),
            "/review D000123"
        );
        assert_eq!(
            normalize_slack_command_text("/export", Some("D000123")),
            "/export D000123"
        );
        assert_eq!(
            normalize_slack_command_text("/unknown", Some("what needs review?")),
            "/help"
        );
    }

    #[test]
    fn slack_manifest_lists_short_commands_only() {
        let manifest = std::fs::read_to_string("docs/slack-app-manifest.yaml")
            .expect("slack manifest should be readable");
        for command in [
            "/help",
            "/overview",
            "/documents",
            "/pending",
            "/ready",
            "/last",
            "/why",
            "/review",
            "/retry",
            "/export",
        ] {
            assert!(manifest.contains(&format!("command: {command}")));
        }
        assert!(!manifest.contains("command: /status"));
        assert!(!manifest.contains("command: /finelor"));
    }

    #[test]
    fn slack_channel_text_addressing_detects_mentions() {
        assert!(slack_text_addresses_app(Some("<@U123> please import this")));
        assert!(!slack_text_addresses_app(Some("please import this")));
        assert!(!slack_text_addresses_app(None));
    }
}
