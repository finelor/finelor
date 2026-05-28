use std::time::Duration;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use serde_json::json;
use teloxide::prelude::*;
use teloxide::types::{
    BotCommand, CallbackQuery, ChatAction, ChatKind, InlineKeyboardButton as KeyboardButton,
    InlineKeyboardMarkup, InputFile, Message, ParseMode, PublicChatKind, Update, UpdateKind,
};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::db;
use crate::ingestion::{self, SavedDocumentRef};
use crate::integrations::telegram::{TelegramNotifier, escape_markdown};
use crate::messaging::contracts::{
    ActionButton, AgentInboundMessage, GatewayAttachment, GatewayMessageFormat, MessageSource,
};
use crate::messaging::dispatch::OutboundChannelAdapter;
use crate::messaging::gateway::{AgentGatewayState, encode_document_action, parse_document_action};
use crate::messaging::intents::slash_command_menu;
use crate::messaging::interventions::InterventionTarget;
use crate::web::events::{
    AppEvent, TelegramConnectStatusKind, get_telegram_connect_session,
    update_telegram_connect_session_status,
};

pub async fn run_telegram_bot(
    bot: Bot,
    state: AgentGatewayState,
    clear_webhook: impl Future<Output = anyhow::Result<()>>,
) -> anyhow::Result<()> {
    use teloxide::dispatching::{Dispatcher, UpdateFilterExt};

    clear_webhook.await?;
    register_telegram_command_menu(&bot).await?;

    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(handle_telegram_message))
        .branch(Update::filter_callback_query().endpoint(handle_telegram_callback_query));

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

pub async fn register_telegram_command_menu(bot: &Bot) -> anyhow::Result<()> {
    let commands = telegram_command_menu();
    bot.set_my_commands(commands).await?;
    info!("Registered Telegram bot command menu");
    Ok(())
}

#[derive(Clone)]
pub struct TelegramOutboundAdapter {
    bot: Bot,
}

impl TelegramOutboundAdapter {
    pub fn new(bot: Bot) -> Self {
        Self { bot }
    }
}

#[async_trait::async_trait]
impl OutboundChannelAdapter for TelegramOutboundAdapter {
    async fn send_gateway_response(
        &self,
        target: &InterventionTarget,
        response: crate::messaging::contracts::GatewayMessageResponse,
    ) -> anyhow::Result<()> {
        let chat_id = target
            .channel_identifier
            .parse::<i64>()
            .map_err(|err| anyhow::anyhow!("invalid Telegram chat id: {err}"))?;

        send_gateway_response_to_chat(&self.bot, ChatId(chat_id), response).await?;
        Ok(())
    }
}

fn telegram_command_menu() -> Vec<BotCommand> {
    slash_command_menu()
        .iter()
        .map(|command| BotCommand::new(command.name, command.description))
        .collect()
}

pub async fn handle_telegram_update(
    bot: Bot,
    update: Update,
    state: AgentGatewayState,
) -> Result<(), teloxide::RequestError> {
    match update.kind {
        UpdateKind::Message(message) => handle_telegram_message(bot, message, state).await,
        UpdateKind::CallbackQuery(callback_query) => {
            handle_telegram_callback_query(bot, callback_query, state).await
        }
        other => {
            info!(update_kind = ?other, "Ignoring unsupported Telegram webhook update");
            Ok(())
        }
    }
}

pub async fn handle_telegram_message(
    bot: Bot,
    msg: Message,
    state: AgentGatewayState,
) -> Result<(), teloxide::RequestError> {
    let chat_id = msg.chat.id;
    info!(
        chat_id = chat_id.0,
        has_text = msg.text().is_some(),
        has_photo = msg.photo().is_some(),
        has_document = msg.document().is_some(),
        "Telegram message received"
    );

    if let Some(text) = msg.text() {
        if text.strip_prefix("/start ").is_some() {
            return handle_text_command(bot, msg.clone(), text, state).await;
        }

        return handle_agent_chat_text(bot, msg.clone(), text, state).await;
    }

    if let Some(photo) = msg.photo() {
        return handle_photo(bot, msg.clone(), photo.to_vec(), state).await;
    }

    if let Some(document) = msg.document() {
        return handle_document(bot, msg.clone(), document.clone(), state).await;
    }

    bot.send_message(
        chat_id,
        "Unsupported message type. Please send a text command or upload a photo/document.",
    )
    .await?;

    Ok(())
}

async fn handle_agent_chat_text(
    bot: Bot,
    msg: Message,
    text: &str,
    state: AgentGatewayState,
) -> Result<(), teloxide::RequestError> {
    let chat_id = msg.chat.id;
    let source = build_text_message_source(&msg);
    let typing = start_typing_indicator(bot.clone(), chat_id);
    let response = state
        .handle_inbound_message(AgentInboundMessage::Text {
            source,
            text: text.to_string(),
        })
        .await;
    typing.stop().await;

    match response {
        Ok(response) => {
            for attachment in &response.attachments {
                if let GatewayAttachment::StoredMedia {
                    channel_type,
                    external_file_id,
                    mime_type,
                    filename,
                } = attachment
                    && let Err(err) = send_stored_gateway_media(
                        &bot,
                        chat_id,
                        channel_type,
                        external_file_id,
                        mime_type,
                        filename.as_deref(),
                    )
                    .await
                {
                    warn!(
                        error = %err,
                        chat_id = chat_id.0,
                        channel_type = channel_type,
                        "Failed to send stored gateway media"
                    );
                }
            }

            send_gateway_text(
                &bot,
                chat_id,
                &response.message,
                response.format,
                response.buttons,
            )
            .await?;

            for attachment in response.attachments {
                if let GatewayAttachment::LocalFile { path } = attachment {
                    bot.send_document(chat_id, InputFile::file(path)).await?;
                }
            }
        }
        Err(err) => {
            error!(error = %err, chat_id = chat_id.0, "Agent chat failed");
            bot.send_message(
                chat_id,
                "I could not answer that right now. Please try again in a moment.",
            )
            .await?;
        }
    }

    Ok(())
}

async fn send_stored_gateway_media(
    bot: &Bot,
    chat_id: ChatId,
    channel_type: &str,
    file_id: &str,
    mime_type: &str,
    filename: Option<&str>,
) -> Result<(), teloxide::RequestError> {
    if !channel_type.eq_ignore_ascii_case("TELEGRAM") {
        warn!(
            chat_id = chat_id.0,
            channel_type = channel_type,
            "Skipping stored media from unsupported channel"
        );
        return Ok(());
    }

    let input = InputFile::file_id(file_id.to_string());
    if telegram_media_is_photo(mime_type) {
        bot.send_photo(chat_id, input).await?;
    } else {
        let request = bot.send_document(chat_id, input);
        let request = if let Some(filename) = filename {
            request.caption(filename.to_string())
        } else {
            request
        };
        request.await?;
    }

    Ok(())
}

fn telegram_media_is_photo(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "image/jpeg" | "image/jpg" | "image/png" | "image/webp"
    )
}

fn build_text_message_source(msg: &Message) -> MessageSource {
    MessageSource {
        channel: db::ChannelType::Telegram,
        channel_identifier: msg.chat.id.0.to_string(),
        profile_identifier: msg.from.as_ref().map(|user| user.id.0.to_string()),
        message_id: Some(msg.id.0.to_string()),
        source_timestamp: None,
        metadata: json!({
            "chat_type": telegram_chat_type_from_kind(&msg.chat.kind),
        }),
    }
}

fn telegram_chat_type_from_kind(kind: &ChatKind) -> &'static str {
    match kind {
        ChatKind::Private(_) => "private",
        ChatKind::Public(chat) => match &chat.kind {
            PublicChatKind::Channel(_) => "channel",
            PublicChatKind::Group(_) => "group",
            PublicChatKind::Supergroup(_) => "supergroup",
        },
    }
}

struct TypingIndicator {
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

impl TypingIndicator {
    async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.task.await;
    }
}

fn start_typing_indicator(bot: Bot, chat_id: ChatId) -> TypingIndicator {
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(4));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(err) = bot.send_chat_action(chat_id, ChatAction::Typing).await {
                        warn!(error = %err, chat_id = chat_id.0, "Failed to send Telegram typing action");
                    }
                }
                _ = &mut stop_rx => break,
            }
        }
    });

    TypingIndicator {
        stop: stop_tx,
        task,
    }
}

async fn handle_text_command(
    bot: Bot,
    msg: Message,
    text: &str,
    state: AgentGatewayState,
) -> Result<(), teloxide::RequestError> {
    let chat_id = msg.chat.id;
    info!(
        chat_id = chat_id.0,
        command = text,
        "Telegram text command received"
    );
    if let Some(start_param) = text.strip_prefix("/start ").map(str::trim) {
        let replay_ttl_seconds = 900_u64;
        let session = match get_telegram_connect_session(&state.ephemeral_store, start_param).await
        {
            Ok(Some(session)) => session,
            Ok(None) => {
                warn!(
                    chat_id = chat_id.0,
                    "Telegram opaque connect token missing or expired"
                );
                bot.send_message(
                    chat_id,
                    "❌ This connect link is invalid or expired. Generate a new one from Settings → Channels.",
                )
                .await?;
                return Ok(());
            }
            Err(err) => {
                error!(error = %err, chat_id = chat_id.0, "Failed to load Telegram connect session");
                bot.send_message(
                    chat_id,
                    "❌ Telegram connect is temporarily unavailable. Please try again in a minute.",
                )
                .await?;
                return Ok(());
            }
        };
        let connect_id = session.connect_id.clone();
        info!(chat_id = chat_id.0, connect_id = %connect_id, "Telegram connect start received");

        let response = if session.status == TelegramConnectStatusKind::Pending
            && chrono::Utc::now().timestamp() <= session.expires_at
        {
            info!(
                chat_id = chat_id.0,
                connect_id = %connect_id,
                workspace_id = %session.workspace_id,
                user_id = %session.user_id,
                "Telegram opaque connect token loaded"
            );

            let metadata = build_telegram_connect_metadata(&msg, session.user_id);
            match crate::web::server::channels::connect_telegram_for_workspace_with_metadata(
                &state.pool,
                session.workspace_id,
                chat_id.0,
                Some(metadata),
            )
            .await
            {
                Ok(message) => {
                    info!(
                        chat_id = chat_id.0,
                        connect_id = %connect_id,
                        workspace_id = %session.workspace_id,
                        result = %message,
                        "Telegram channel identity connected"
                    );
                    let status_kind = if message.contains("already connected") {
                        TelegramConnectStatusKind::AlreadyConnected
                    } else {
                        TelegramConnectStatusKind::Connected
                    };
                    if let Err(err) = update_telegram_connect_session_status(
                        &state.ephemeral_store,
                        session.clone(),
                        status_kind.clone(),
                        None,
                        replay_ttl_seconds as usize,
                    )
                    .await
                    {
                        error!(error = %err, connect_id = %connect_id, "Failed to write Telegram connect status");
                    }
                    publish_telegram_connect_event(
                        &state,
                        session.workspace_id,
                        &connect_id,
                        status_kind,
                        chat_id.0,
                    );
                    publish_channels_changed_event(&state, session.workspace_id);

                    "✅ Telegram connected to your company. You can now send receipts directly here.".to_string()
                }
                Err(err) => {
                    error!(
                        error = %err,
                        chat_id = chat_id.0,
                        connect_id = %connect_id,
                        workspace_id = %session.workspace_id,
                        "Telegram channel identity connection failed"
                    );
                    if let Err(store_err) = update_telegram_connect_session_status(
                        &state.ephemeral_store,
                        session.clone(),
                        TelegramConnectStatusKind::Failed,
                        Some("Failed to connect Telegram channel.".to_string()),
                        replay_ttl_seconds as usize,
                    )
                    .await
                    {
                        error!(error = %store_err, connect_id = %connect_id, "Failed to write Telegram connect failure status");
                    }
                    state.events.publish(AppEvent::new(
                        session.workspace_id,
                        "telegram_connect.failed",
                        json!({
                            "connect_id": connect_id,
                            "channel_type": "TELEGRAM",
                        }),
                    ));
                    format!("❌ Failed to connect Telegram: {}", err)
                }
            }
        } else {
            warn!(
                chat_id = chat_id.0,
                connect_id = %connect_id,
                status = ?session.status,
                "Telegram opaque connect token used or expired"
            );
            "❌ This connect link was already used or expired. Generate a new one from Settings → Channels.".to_string()
        };
        bot.send_message(chat_id, response).await?;
        return Ok(());
    }

    bot.send_message(
        chat_id,
        "Open a current connection link from Settings > Channels, or send /help once this chat is connected.",
    )
    .await?;
    Ok(())
}

pub async fn handle_telegram_callback_query(
    bot: Bot,
    query: CallbackQuery,
    state: AgentGatewayState,
) -> Result<(), teloxide::RequestError> {
    let callback_data = match query.data.as_deref() {
        Some(value) => value,
        None => return Ok(()),
    };
    let interaction_chat_id = query
        .message
        .as_ref()
        .map(|message| message.chat().id.0)
        .unwrap_or(query.from.id.0 as i64);
    let source_profile_identifier = query.from.id.0.to_string();
    let action = match parse_document_action(callback_data) {
        Ok(action) => action,
        Err(err) => {
            error!(error = %err, callback = callback_data, "Failed to parse Telegram callback action");
            return Ok(());
        }
    };
    let source = MessageSource {
        channel: db::ChannelType::Telegram,
        channel_identifier: interaction_chat_id.to_string(),
        profile_identifier: Some(source_profile_identifier),
        message_id: query
            .message
            .as_ref()
            .map(|message| message.id().0.to_string()),
        source_timestamp: None,
        metadata: json!({
            "callback_id": query.id.to_string(),
            "chat_type": query
                .message
                .as_ref()
                .map(|message| telegram_chat_type_from_kind(&message.chat().kind))
                .unwrap_or("chat"),
        }),
    };

    let notifier = TelegramNotifier::new(state.config.messaging.telegram.bot_token.clone());
    let response = match state.handle_document_action(&source, action).await {
        Ok(result) => result,
        Err(err) => {
            error!(error = %err, callback = callback_data, "Failed to process callback query");
            crate::messaging::contracts::GatewayActionResponse {
                acknowledgement: "Action failed".to_string(),
                message: "Failed to process that action.".to_string(),
                format: GatewayMessageFormat::PlainText,
                attachments: Vec::new(),
                buttons: None,
            }
        }
    };

    if let Err(err) = notifier
        .answer_callback_query(&query.id, Some(&response.acknowledgement))
        .await
    {
        error!(error = %err, "Failed to answer callback query");
    }

    if let Some(message) = query.message {
        send_gateway_response_to_chat(
            &bot,
            message.chat().id,
            crate::messaging::contracts::GatewayMessageResponse {
                message: response.message,
                format: response.format,
                attachments: response.attachments,
                buttons: response.buttons,
            },
        )
        .await?;
    }

    Ok(())
}

async fn send_gateway_response_to_chat(
    bot: &Bot,
    chat_id: ChatId,
    response: crate::messaging::contracts::GatewayMessageResponse,
) -> Result<(), teloxide::RequestError> {
    for attachment in &response.attachments {
        if let GatewayAttachment::StoredMedia {
            channel_type,
            external_file_id,
            mime_type,
            filename,
        } = attachment
        {
            send_stored_gateway_media(
                bot,
                chat_id,
                channel_type,
                external_file_id,
                mime_type,
                filename.as_deref(),
            )
            .await?;
        }
    }

    if !response.message.trim().is_empty() {
        send_gateway_text(
            bot,
            chat_id,
            &response.message,
            response.format,
            response.buttons,
        )
        .await?;
    }

    for attachment in response.attachments {
        if let GatewayAttachment::LocalFile { path } = attachment {
            bot.send_document(chat_id, InputFile::file(path)).await?;
        }
    }

    Ok(())
}

async fn send_gateway_text(
    bot: &Bot,
    chat_id: ChatId,
    message: &str,
    format: GatewayMessageFormat,
    buttons: Option<Vec<Vec<ActionButton>>>,
) -> Result<(), teloxide::RequestError> {
    let rendered = render_gateway_message_for_telegram(message, format);
    let keyboard = buttons.map(to_telegram_action_keyboard);
    let request = bot.send_message(chat_id, rendered.text.clone());
    let request = if let Some(parse_mode) = rendered.parse_mode {
        request.parse_mode(parse_mode)
    } else {
        request
    };
    let request = if let Some(keyboard) = keyboard.clone() {
        request.reply_markup(keyboard)
    } else {
        request
    };

    if let Err(err) = request.await {
        if rendered.parse_mode.is_some() {
            warn!(
                error = %err,
                chat_id = chat_id.0,
                "Formatted Telegram message failed; retrying as plain text"
            );
            let fallback = telegram_plain_text_fallback(message);
            let request = bot.send_message(chat_id, fallback);
            let request = if let Some(keyboard) = keyboard {
                request.reply_markup(keyboard)
            } else {
                request
            };
            request.await?;
            return Ok(());
        }

        return Err(err);
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TelegramRenderedMessage {
    text: String,
    parse_mode: Option<ParseMode>,
}

fn render_gateway_message_for_telegram(
    message: &str,
    format: GatewayMessageFormat,
) -> TelegramRenderedMessage {
    match format {
        GatewayMessageFormat::PlainText => TelegramRenderedMessage {
            text: telegram_plain_text_fallback(message),
            parse_mode: None,
        },
        GatewayMessageFormat::Markdown => TelegramRenderedMessage {
            text: markdown_to_telegram_html(message),
            parse_mode: Some(ParseMode::Html),
        },
    }
}

fn markdown_to_telegram_html(markdown: &str) -> String {
    let normalized = normalize_markdown_tables(markdown);
    let parser = Parser::new_ext(&normalized, Options::empty());
    let mut output = String::new();
    let mut list_stack: Vec<Option<u64>> = Vec::new();

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => start_block(&mut output),
                Tag::Heading { .. } => {
                    start_block(&mut output);
                    output.push_str("<b>");
                }
                Tag::Strong => output.push_str("<b>"),
                Tag::Emphasis => output.push_str("<i>"),
                Tag::CodeBlock(_) => {
                    start_block(&mut output);
                    output.push_str("<pre>");
                }
                Tag::Link { dest_url, .. } => {
                    output.push_str("<a href=\"");
                    output.push_str(&escape_telegram_html_attr(&dest_url));
                    output.push_str("\">");
                }
                Tag::List(start) => {
                    start_block(&mut output);
                    list_stack.push(start);
                }
                Tag::Item => {
                    start_line(&mut output);
                    if let Some(current) = list_stack.last_mut() {
                        match current {
                            Some(index) => {
                                output.push_str(&format!("{}. ", *index));
                                *index += 1;
                            }
                            None => output.push_str("- "),
                        }
                    }
                }
                Tag::BlockQuote(_) => {
                    start_block(&mut output);
                    output.push_str("<blockquote>");
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => end_block(&mut output),
                TagEnd::Heading(_) => {
                    output.push_str("</b>");
                    end_block(&mut output);
                }
                TagEnd::Strong => output.push_str("</b>"),
                TagEnd::Emphasis => output.push_str("</i>"),
                TagEnd::CodeBlock => {
                    output.push_str("</pre>");
                    end_block(&mut output);
                }
                TagEnd::Link => output.push_str("</a>"),
                TagEnd::List(_) => {
                    list_stack.pop();
                    end_block(&mut output);
                }
                TagEnd::Item => output.push('\n'),
                TagEnd::BlockQuote(_) => {
                    output.push_str("</blockquote>");
                    end_block(&mut output);
                }
                _ => {}
            },
            Event::Text(text) => output.push_str(&escape_telegram_html(&text)),
            Event::Code(code) => {
                output.push_str("<code>");
                output.push_str(&escape_telegram_html(&code));
                output.push_str("</code>");
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                output.push_str(&escape_telegram_html(&html));
            }
            Event::SoftBreak | Event::HardBreak => output.push('\n'),
            Event::Rule => {
                start_block(&mut output);
                output.push_str("---");
                end_block(&mut output);
            }
            Event::TaskListMarker(checked) => {
                output.push_str(if checked { "[x] " } else { "[ ] " });
            }
            Event::FootnoteReference(reference) => {
                output.push('[');
                output.push_str(&escape_telegram_html(&reference));
                output.push(']');
            }
            Event::InlineMath(math) | Event::DisplayMath(math) => {
                output.push_str(&escape_telegram_html(&math));
            }
        }
    }

    output.trim().to_string()
}

fn telegram_plain_text_fallback(markdown: &str) -> String {
    let normalized = normalize_markdown_tables(markdown);
    let parser = Parser::new_ext(&normalized, Options::empty());
    let mut output = String::new();
    let mut list_stack: Vec<Option<u64>> = Vec::new();

    for event in parser {
        match event {
            Event::Start(Tag::Paragraph | Tag::Heading { .. } | Tag::CodeBlock(_)) => {
                start_block(&mut output);
            }
            Event::Start(Tag::List(start)) => {
                start_block(&mut output);
                list_stack.push(start);
            }
            Event::Start(Tag::Item) => {
                start_line(&mut output);
                if let Some(current) = list_stack.last_mut() {
                    match current {
                        Some(index) => {
                            output.push_str(&format!("{}. ", *index));
                            *index += 1;
                        }
                        None => output.push_str("- "),
                    }
                }
            }
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock) => {
                end_block(&mut output);
            }
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
                end_block(&mut output);
            }
            Event::End(TagEnd::Item) => output.push('\n'),
            Event::Text(text)
            | Event::Code(text)
            | Event::Html(text)
            | Event::InlineHtml(text)
            | Event::InlineMath(text)
            | Event::DisplayMath(text) => output.push_str(&text),
            Event::SoftBreak | Event::HardBreak => output.push('\n'),
            Event::Rule => {
                start_block(&mut output);
                output.push_str("---");
                end_block(&mut output);
            }
            Event::TaskListMarker(checked) => {
                output.push_str(if checked { "[x] " } else { "[ ] " });
            }
            Event::FootnoteReference(reference) => {
                output.push('[');
                output.push_str(&reference);
                output.push(']');
            }
            _ => {}
        }
    }

    output.trim().to_string()
}

fn normalize_markdown_tables(markdown: &str) -> String {
    let lines = markdown.lines().collect::<Vec<_>>();
    let mut output = Vec::with_capacity(lines.len());
    let mut index = 0;

    while index < lines.len() {
        if index + 1 < lines.len()
            && looks_like_table_row(lines[index])
            && looks_like_table_separator(lines[index + 1])
        {
            while index < lines.len() && looks_like_table_row(lines[index]) {
                if !looks_like_table_separator(lines[index]) {
                    output.push(table_row_to_text(lines[index]));
                }
                index += 1;
            }
            continue;
        }

        output.push(lines[index].to_string());
        index += 1;
    }

    output.join("\n")
}

fn looks_like_table_row(line: &str) -> bool {
    line.matches('|').count() >= 2
}

fn looks_like_table_separator(line: &str) -> bool {
    let trimmed = line.trim().trim_matches('|').trim();
    !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|character| matches!(character, '-' | ':' | '|' | ' '))
        && trimmed.contains('-')
}

fn table_row_to_text(line: &str) -> String {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .filter(|cell| !cell.is_empty())
        .collect::<Vec<_>>()
        .join(" - ")
}

fn escape_telegram_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_telegram_html_attr(text: &str) -> String {
    escape_telegram_html(text).replace('"', "&quot;")
}

fn start_block(output: &mut String) {
    if !output.is_empty() && !output.ends_with("\n\n") {
        if output.ends_with('\n') {
            output.push('\n');
        } else {
            output.push_str("\n\n");
        }
    }
}

fn start_line(output: &mut String) {
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
}

fn end_block(output: &mut String) {
    while output.ends_with(' ') {
        output.pop();
    }
    if !output.ends_with("\n\n") {
        if output.ends_with('\n') {
            output.push('\n');
        } else {
            output.push_str("\n\n");
        }
    }
}

fn to_telegram_action_keyboard(buttons: Vec<Vec<ActionButton>>) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(buttons.into_iter().map(|row| {
        row.into_iter()
            .map(|button| {
                KeyboardButton::callback(button.label, encode_document_action(button.action))
            })
            .collect::<Vec<_>>()
    }))
}

fn build_telegram_connect_metadata(msg: &Message, connected_by_user_id: i64) -> serde_json::Value {
    let (chat_type, title, username, first_name, last_name) = match &msg.chat.kind {
        ChatKind::Private(chat) => (
            "private",
            None,
            chat.username.clone(),
            chat.first_name.clone(),
            chat.last_name.clone(),
        ),
        ChatKind::Public(chat) => {
            let (chat_type, username) = match &chat.kind {
                PublicChatKind::Channel(channel) => ("channel", channel.username.clone()),
                PublicChatKind::Group(_) => ("group", None),
                PublicChatKind::Supergroup(supergroup) => {
                    ("supergroup", supergroup.username.clone())
                }
            };
            (chat_type, chat.title.clone(), username, None, None)
        }
    };

    let display_name = title
        .clone()
        .or_else(|| {
            let first = first_name.as_deref()?.trim();
            let last = last_name.as_deref().unwrap_or_default().trim();
            if last.is_empty() {
                Some(first.to_string())
            } else {
                Some(format!("{first} {last}"))
            }
        })
        .or_else(|| username.as_ref().map(|value| format!("@{value}")))
        .unwrap_or_else(|| "Telegram chat".to_string());

    let user = msg.from.as_ref();

    json!({
        "source": "telegram_connect",
        "connected_at": chrono::Utc::now().to_rfc3339(),
        "connected_by_user_id": connected_by_user_id.to_string(),
        "chat": {
            "id": msg.chat.id.0,
            "type": chat_type,
            "title": title,
            "username": username,
            "first_name": first_name,
            "last_name": last_name,
            "display_name": display_name,
        },
        "user": user.map(|user| json!({
            "id": user.id.0,
            "username": user.username,
            "first_name": user.first_name,
            "last_name": user.last_name,
            "language_code": user.language_code,
            "is_bot": user.is_bot,
        })),
    })
}

fn publish_telegram_connect_event(
    state: &AgentGatewayState,
    workspace_id: Uuid,
    connect_id: &str,
    status: TelegramConnectStatusKind,
    chat_id: i64,
) {
    info!(
        workspace_id = %workspace_id,
        connect_id = %connect_id,
        chat_id = chat_id,
        status = ?status,
        "Publishing Telegram connect SSE event"
    );
    state.events.publish(AppEvent::new(
        workspace_id,
        "telegram_connect.connected",
        json!({
            "connect_id": connect_id,
            "status": status,
            "channel_type": "TELEGRAM",
            "chat_id": chat_id.to_string(),
        }),
    ));
}

fn publish_channels_changed_event(state: &AgentGatewayState, workspace_id: Uuid) {
    info!(
        workspace_id = %workspace_id,
        channel_type = "TELEGRAM",
        "Publishing channels changed SSE event"
    );
    state.events.publish(AppEvent::new(
        workspace_id,
        "channels.changed",
        json!({
            "channel_type": "TELEGRAM",
        }),
    ));
}

async fn handle_photo(
    bot: Bot,
    msg: Message,
    photo: Vec<teloxide::types::PhotoSize>,
    state: AgentGatewayState,
) -> Result<(), teloxide::RequestError> {
    let chat_id = msg.chat.id;
    bot.send_message(chat_id, "📸 Received photo! Processing...")
        .await?;

    let file_id = photo
        .last()
        .map(|size| size.file.id.clone())
        .unwrap_or_default();

    if file_id.is_empty() {
        bot.send_message(chat_id, "⚠️ Could not access the photo file.")
            .await?;
        return Ok(());
    }

    match download_and_save_file(&bot, &file_id, &state, &msg, "image/jpeg", None).await {
        Ok(saved) => {
            bot.send_message(
                chat_id,
                format! {
                    "✅ Photo saved successfully! Reference: {}",
                    escape_markdown(&saved.short_ref)
                },
            )
            .await?;
        }
        Err(e) => {
            error!(error = %e, "Failed to process photo");
            bot.send_message(chat_id, "❌ Failed to process photo. Please try again.")
                .await?;
        }
    }

    Ok(())
}

async fn handle_document(
    bot: Bot,
    msg: Message,
    doc: teloxide::types::Document,
    state: AgentGatewayState,
) -> Result<(), teloxide::RequestError> {
    let chat_id = msg.chat.id;
    bot.send_message(chat_id, "📄 Received document! Processing...")
        .await?;

    let file_id = doc.file.id.clone();
    let mime_type = doc
        .mime_type
        .as_ref()
        .map(|m| m.to_string())
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let filename = doc
        .file_name
        .clone()
        .unwrap_or_else(|| "unknown.pdf".to_string());

    match download_and_save_file(
        &bot,
        &file_id,
        &state,
        &msg,
        &mime_type,
        Some(filename.clone()),
    )
    .await
    {
        Ok(saved) => {
            bot.send_message(
                chat_id,
                format! {
                    "✅ Document '{}' saved successfully! Reference: {}",
                    escape_markdown(&filename),
                    escape_markdown(&saved.short_ref)
                },
            )
            .await?;
        }
        Err(e) => {
            error!(error = %e, "Failed to process document");
            bot.send_message(chat_id, "❌ Failed to process document. Please try again.")
                .await?;
        }
    }

    Ok(())
}

async fn download_and_save_file(
    bot: &Bot,
    file_id: &str,
    state: &AgentGatewayState,
    msg: &Message,
    mime_type: &str,
    original_filename: Option<String>,
) -> anyhow::Result<SavedDocumentRef> {
    let file = bot.get_file(file_id).await?;
    let dest = file.path.as_str();

    if dest.is_empty() {
        anyhow::bail!("File path is empty");
    }

    let extension = if mime_type == "image/jpeg" || mime_type == "image/jpg" {
        "jpg"
    } else if mime_type == "image/png" {
        "png"
    } else if mime_type == "application/pdf" {
        "pdf"
    } else {
        "bin"
    };

    let upload_token = Uuid::new_v4().to_string();
    let filename = build_stored_upload_filename(&upload_token, msg.chat.id.0, msg.id, extension);

    let token = bot.token();
    let download_url = format!("https://api.telegram.org/file/bot{}/{}", token, dest);

    let response = reqwest::get(&download_url).await?;
    let file_data: Vec<u8> = response.bytes().await?.to_vec();

    let external_artifact_id = format!("tg_{}_{}", msg.chat.id.0, msg.id.0);
    let channel_identifier = msg.chat.id.0.to_string();
    let profile_identifier = msg.from.as_ref().map(|u| u.id.0.to_string());
    let artifact_metadata = json!({
        "chat_id": msg.chat.id.0,
        "message_id": msg.id.0,
        "file_id": file_id,
        "source_identity": channel_identifier,
        "profile_identifier": profile_identifier,
        "from": msg.from.as_ref().map(|user| json!({
            "id": user.id.0,
            "username": user.username,
            "first_name": user.first_name,
            "last_name": user.last_name,
            "language_code": user.language_code,
            "is_bot": user.is_bot,
        })),
    });

    let source_timestamp = Some(msg.date.to_rfc3339());

    ingestion::ingest_document(
        &state.pool,
        state.config.as_ref(),
        &state.queue_producer,
        Some(&state.events),
        ingestion::IngestionInput {
            file_bytes: file_data,
            filename,
            mime_type: mime_type.to_string(),
            document_type: "INVOICE".to_string(),
            artifact: ingestion::DocumentArtifactInput {
                channel_type: "TELEGRAM".to_string(),
                channel_identifier,
                profile_identifier,
                external_artifact_id: Some(external_artifact_id),
                source_timestamp,
                original_filename,
                metadata: artifact_metadata,
            },
        },
    )
    .await
}

fn build_stored_upload_filename(
    upload_token: &str,
    chat_id: i64,
    message_id: teloxide::types::MessageId,
    extension: &str,
) -> String {
    let timestamp = chrono::Utc::now().timestamp_millis();
    format!(
        "{}_{}_{}_{}.{}",
        timestamp, chat_id, message_id.0, upload_token, extension
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_markdown_to_telegram_html() {
        let rendered = render_gateway_message_for_telegram(
            "**Status**\n\n- `D000057` needs <review> & approval",
            GatewayMessageFormat::Markdown,
        );

        assert_eq!(rendered.parse_mode, Some(ParseMode::Html));
        assert!(rendered.text.contains("<b>Status</b>"));
        assert!(
            rendered
                .text
                .contains("- <code>D000057</code> needs &lt;review&gt; &amp; approval")
        );
    }

    #[test]
    fn renders_links_with_escaped_label_and_url() {
        let rendered = markdown_to_telegram_html("[open <doc>](https://example.com?a=1&b=\"x\")");

        assert_eq!(
            rendered,
            "<a href=\"https://example.com?a=1&amp;b=&quot;x&quot;\">open &lt;doc&gt;</a>"
        );
    }

    #[test]
    fn normalizes_markdown_tables_for_telegram() {
        let rendered = markdown_to_telegram_html(
            "| Status | Count |\n| --- | ---: |\n| Pending review | 3 |\n| Export ready | 2 |",
        );

        assert!(rendered.contains("Status - Count"));
        assert!(rendered.contains("Pending review - 3"));
        assert!(!rendered.contains("| ---"));
    }

    #[test]
    fn escapes_raw_html_from_model() {
        let rendered = markdown_to_telegram_html("<script>alert('x')</script>");

        assert_eq!(rendered, "&lt;script&gt;alert('x')&lt;/script&gt;");
    }

    #[test]
    fn plain_text_fallback_removes_common_markdown() {
        let fallback = telegram_plain_text_fallback("**Status**\n\n- `D000057` needs review");

        assert_eq!(fallback, "Status\n\n- D000057 needs review");
    }

    #[test]
    fn telegram_media_is_photo_for_supported_image_mime_types() {
        assert!(telegram_media_is_photo("image/jpeg"));
        assert!(telegram_media_is_photo("image/jpg"));
        assert!(telegram_media_is_photo("image/png"));
        assert!(telegram_media_is_photo("image/webp"));
    }

    #[test]
    fn telegram_media_is_document_for_non_image_mime_types() {
        assert!(!telegram_media_is_photo("application/pdf"));
        assert!(!telegram_media_is_photo("application/octet-stream"));
    }
}
