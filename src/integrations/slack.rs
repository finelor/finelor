#[cfg(feature = "ssr")]
use leptos::prelude::ServerFnError;
use serde::Deserialize;

#[derive(Deserialize)]
struct SlackAuthTestResponse {
    ok: bool,
    team: Option<String>,
    team_id: Option<String>,
    url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SlackChannelResolution {
    pub channel_id: String,
    pub channel_name: String,
    pub channel_type: String,
    pub team_id: Option<String>,
}

#[derive(Deserialize)]
struct SlackConversationsListResponse {
    ok: bool,
    channels: Option<Vec<SlackConversation>>,
    response_metadata: Option<SlackResponseMetadata>,
}

#[derive(Deserialize)]
struct SlackResponseMetadata {
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
struct SlackConversation {
    id: String,
    name: Option<String>,
    is_private: Option<bool>,
}

#[derive(Deserialize)]
struct SlackConversationsInfoResponse {
    ok: bool,
    channel: Option<SlackConversation>,
}

#[cfg(feature = "ssr")]
pub async fn fetch_workspace_info(
    bot_token: &str,
) -> Result<(Option<String>, Option<String>, Option<String>), ServerFnError> {
    let response = reqwest::Client::new()
        .post("https://slack.com/api/auth.test")
        .bearer_auth(bot_token)
        .send()
        .await
        .map_err(|e| ServerFnError::new(format!("Slack auth.test failed: {}", e)))?;
    if !response.status().is_success() {
        return Ok((None, None, None));
    }
    let payload = response
        .json::<SlackAuthTestResponse>()
        .await
        .map_err(|e| ServerFnError::new(format!("Slack auth.test parse failed: {}", e)))?;
    if !payload.ok {
        return Ok((None, None, None));
    }
    Ok((payload.team, payload.team_id, payload.url))
}

#[cfg(feature = "ssr")]
pub async fn fetch_channel_name(
    bot_token: &str,
    channel_id: &str,
) -> Result<Option<String>, ServerFnError> {
    Ok(fetch_channel_info(bot_token, channel_id)
        .await?
        .map(|resolved| resolved.channel_name))
}

#[cfg(feature = "ssr")]
pub async fn fetch_channel_info(
    bot_token: &str,
    channel_id: &str,
) -> Result<Option<SlackChannelResolution>, ServerFnError> {
    let response = reqwest::Client::new()
        .get("https://slack.com/api/conversations.info")
        .bearer_auth(bot_token)
        .query(&[("channel", channel_id)])
        .send()
        .await
        .map_err(|e| ServerFnError::new(format!("Slack conversations.info failed: {}", e)))?;
    if !response.status().is_success() {
        return Ok(None);
    }
    let payload = response
        .json::<SlackConversationsInfoResponse>()
        .await
        .map_err(|e| ServerFnError::new(format!("Slack conversations.info parse failed: {}", e)))?;
    if !payload.ok {
        return Ok(None);
    }
    let Some(channel) = payload.channel else {
        return Ok(None);
    };
    let Some(name) = channel
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
    else {
        return Ok(None);
    };
    Ok(Some(SlackChannelResolution {
        channel_id: channel.id,
        channel_name: name,
        channel_type: if channel.is_private.unwrap_or(false) {
            "private".to_string()
        } else {
            "public".to_string()
        },
        team_id: None,
    }))
}

#[cfg(feature = "ssr")]
pub async fn resolve_channel_by_name(
    bot_token: &str,
    channel_name: &str,
) -> Result<Option<SlackChannelResolution>, ServerFnError> {
    let normalized_name = channel_name.trim().trim_start_matches('#').to_ascii_lowercase();
    if normalized_name.is_empty() {
        return Ok(None);
    }

    let client = reqwest::Client::new();
    let mut cursor: Option<String> = None;

    loop {
        let mut query = vec![
            ("types", "public_channel,private_channel".to_string()),
            ("exclude_archived", "true".to_string()),
            ("limit", "200".to_string()),
        ];
        if let Some(ref cursor_value) = cursor {
            query.push(("cursor", cursor_value.clone()));
        }

        let response = client
            .get("https://slack.com/api/conversations.list")
            .bearer_auth(bot_token)
            .query(&query)
            .send()
            .await
            .map_err(|e| ServerFnError::new(format!("Slack conversations.list failed: {}", e)))?;
        if !response.status().is_success() {
            return Ok(None);
        }
        let payload = response
            .json::<SlackConversationsListResponse>()
            .await
            .map_err(|e| {
                ServerFnError::new(format!("Slack conversations.list parse failed: {}", e))
            })?;
        if !payload.ok {
            return Ok(None);
        }

        if let Some(channels) = payload.channels {
            for channel in channels {
                let Some(name) = channel
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                else {
                    continue;
                };
                if name.to_ascii_lowercase() == normalized_name {
                    return Ok(Some(SlackChannelResolution {
                        channel_id: channel.id,
                        channel_name: name,
                        channel_type: if channel.is_private.unwrap_or(false) {
                            "private".to_string()
                        } else {
                            "public".to_string()
                        },
                        team_id: None,
                    }));
                }
            }
        }

        let next_cursor = payload
            .response_metadata
            .and_then(|metadata| metadata.next_cursor)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if next_cursor.is_none() {
            break;
        }
        cursor = next_cursor;
    }

    Ok(None)
}
