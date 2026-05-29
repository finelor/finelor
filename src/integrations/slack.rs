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

#[derive(Debug, Clone)]
pub struct SlackChannelCandidate {
    pub channel_id: String,
    pub channel_name: String,
    pub channel_type: String,
}

#[derive(Debug, Clone)]
pub struct SlackUserResolution {
    pub user_id: String,
    pub username: String,
    pub display_name: String,
    pub dm_channel_id: String,
}

#[derive(Debug, Clone)]
pub struct SlackUserCandidate {
    pub user_id: String,
    pub username: String,
    pub display_name: String,
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

#[derive(Deserialize)]
struct SlackUsersListResponse {
    ok: bool,
    error: Option<String>,
    members: Option<Vec<SlackUser>>,
    response_metadata: Option<SlackResponseMetadata>,
}

#[derive(Deserialize)]
struct SlackUserProfile {
    display_name: Option<String>,
    real_name: Option<String>,
}

#[derive(Deserialize)]
struct SlackUser {
    id: String,
    name: Option<String>,
    deleted: Option<bool>,
    is_bot: Option<bool>,
    profile: Option<SlackUserProfile>,
}

#[derive(Deserialize)]
struct SlackConversationsOpenResponse {
    ok: bool,
    channel: Option<SlackConversationOpenChannel>,
}

#[derive(Deserialize)]
struct SlackConversationOpenChannel {
    id: String,
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
    let normalized_name = channel_name
        .trim()
        .trim_start_matches('#')
        .to_ascii_lowercase();
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

#[cfg(feature = "ssr")]
pub async fn resolve_channel_by_id(
    bot_token: &str,
    channel_id: &str,
) -> Result<Option<SlackChannelResolution>, ServerFnError> {
    fetch_channel_info(bot_token, channel_id).await
}

#[cfg(feature = "ssr")]
pub async fn search_channels(
    bot_token: &str,
    query: &str,
) -> Result<Vec<SlackChannelCandidate>, ServerFnError> {
    let normalized_query = query.trim().trim_start_matches('#').to_ascii_lowercase();
    if normalized_query.is_empty() {
        return Ok(Vec::new());
    }

    let client = reqwest::Client::new();
    let mut cursor: Option<String> = None;
    let mut matches: Vec<SlackChannelCandidate> = Vec::new();

    loop {
        let mut query_params = vec![
            ("types", "public_channel,private_channel".to_string()),
            ("exclude_archived", "true".to_string()),
            ("limit", "200".to_string()),
        ];
        if let Some(ref cursor_value) = cursor {
            query_params.push(("cursor", cursor_value.clone()));
        }

        let response = client
            .get("https://slack.com/api/conversations.list")
            .bearer_auth(bot_token)
            .query(&query_params)
            .send()
            .await
            .map_err(|e| ServerFnError::new(format!("Slack conversations.list failed: {}", e)))?;
        if !response.status().is_success() {
            return Ok(Vec::new());
        }
        let payload = response
            .json::<SlackConversationsListResponse>()
            .await
            .map_err(|e| {
                ServerFnError::new(format!("Slack conversations.list parse failed: {}", e))
            })?;
        if !payload.ok {
            return Ok(Vec::new());
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
                if !name.to_ascii_lowercase().contains(&normalized_query) {
                    continue;
                }
                matches.push(SlackChannelCandidate {
                    channel_id: channel.id,
                    channel_name: name,
                    channel_type: if channel.is_private.unwrap_or(false) {
                        "private".to_string()
                    } else {
                        "public".to_string()
                    },
                });
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

    matches.sort_by(|a, b| {
        let a_exact = a.channel_name.eq_ignore_ascii_case(&normalized_query);
        let b_exact = b.channel_name.eq_ignore_ascii_case(&normalized_query);
        b_exact
            .cmp(&a_exact)
            .then_with(|| a.channel_name.cmp(&b.channel_name))
    });
    matches.truncate(20);
    Ok(matches)
}

#[cfg(feature = "ssr")]
pub async fn resolve_user_by_username(
    bot_token: &str,
    username: &str,
) -> Result<Option<SlackUserResolution>, ServerFnError> {
    let normalized_username = username.trim().trim_start_matches('@').to_ascii_lowercase();
    if normalized_username.is_empty() {
        return Ok(None);
    }

    let client = reqwest::Client::new();
    let mut cursor: Option<String> = None;
    let mut found: Option<(String, String, String)> = None;

    loop {
        let mut query = vec![("limit", "200".to_string())];
        if let Some(ref cursor_value) = cursor {
            query.push(("cursor", cursor_value.clone()));
        }

        let response = client
            .get("https://slack.com/api/users.list")
            .bearer_auth(bot_token)
            .query(&query)
            .send()
            .await
            .map_err(|e| ServerFnError::new(format!("Slack users.list failed: {}", e)))?;
        if !response.status().is_success() {
            return Ok(None);
        }
        let payload = response
            .json::<SlackUsersListResponse>()
            .await
            .map_err(|e| ServerFnError::new(format!("Slack users.list parse failed: {}", e)))?;
        if !payload.ok {
            let error = payload.error.unwrap_or_else(|| "unknown_error".to_string());
            return Err(ServerFnError::new(format!(
                "Slack users.list failed: {error}. Ensure the bot token has users:read scope."
            )));
        }

        if let Some(members) = payload.members {
            for user in members {
                if user.deleted.unwrap_or(false) || user.is_bot.unwrap_or(false) {
                    continue;
                }
                let Some(name) = user
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                else {
                    continue;
                };
                if name.to_ascii_lowercase() != normalized_username {
                    continue;
                }
                let display_name = user
                    .profile
                    .as_ref()
                    .and_then(|profile| profile.display_name.as_deref())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .or_else(|| {
                        user.profile
                            .as_ref()
                            .and_then(|profile| profile.real_name.as_deref())
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                    })
                    .unwrap_or(name.as_str())
                    .to_string();
                found = Some((user.id, name, display_name));
                break;
            }
        }

        if found.is_some() {
            break;
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

    let Some((user_id, resolved_username, display_name)) = found else {
        return Ok(None);
    };
    let Some(dm_channel_id) = open_dm_channel(bot_token, &user_id).await? else {
        return Ok(None);
    };
    Ok(Some(SlackUserResolution {
        user_id,
        username: resolved_username,
        display_name,
        dm_channel_id,
    }))
}

#[cfg(feature = "ssr")]
pub async fn search_users(
    bot_token: &str,
    query: &str,
) -> Result<Vec<SlackUserCandidate>, ServerFnError> {
    let normalized_query = query.trim().trim_start_matches('@').to_ascii_lowercase();
    if normalized_query.is_empty() {
        return Ok(Vec::new());
    }

    let client = reqwest::Client::new();
    let mut cursor: Option<String> = None;
    let mut matches: Vec<SlackUserCandidate> = Vec::new();

    loop {
        let mut query_params = vec![("limit", "200".to_string())];
        if let Some(ref cursor_value) = cursor {
            query_params.push(("cursor", cursor_value.clone()));
        }

        let response = client
            .get("https://slack.com/api/users.list")
            .bearer_auth(bot_token)
            .query(&query_params)
            .send()
            .await
            .map_err(|e| ServerFnError::new(format!("Slack users.list failed: {}", e)))?;
        if !response.status().is_success() {
            return Ok(Vec::new());
        }
        let payload = response
            .json::<SlackUsersListResponse>()
            .await
            .map_err(|e| ServerFnError::new(format!("Slack users.list parse failed: {}", e)))?;
        if !payload.ok {
            let error = payload.error.unwrap_or_else(|| "unknown_error".to_string());
            return Err(ServerFnError::new(format!(
                "Slack users.list failed: {error}. Ensure the bot token has users:read scope."
            )));
        }

        if let Some(members) = payload.members {
            for user in members {
                if user.deleted.unwrap_or(false) || user.is_bot.unwrap_or(false) {
                    continue;
                }
                let Some(username) = user
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                else {
                    continue;
                };
                let display_name = user
                    .profile
                    .as_ref()
                    .and_then(|profile| profile.display_name.as_deref())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .or_else(|| {
                        user.profile
                            .as_ref()
                            .and_then(|profile| profile.real_name.as_deref())
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                    })
                    .unwrap_or(username.as_str())
                    .to_string();
                let username_lower = username.to_ascii_lowercase();
                let display_lower = display_name.to_ascii_lowercase();
                if username_lower.contains(&normalized_query)
                    || display_lower.contains(&normalized_query)
                {
                    matches.push(SlackUserCandidate {
                        user_id: user.id,
                        username,
                        display_name,
                    });
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

    matches.sort_by(|a, b| {
        let a_exact = a.username.eq_ignore_ascii_case(&normalized_query)
            || a.display_name.eq_ignore_ascii_case(&normalized_query);
        let b_exact = b.username.eq_ignore_ascii_case(&normalized_query)
            || b.display_name.eq_ignore_ascii_case(&normalized_query);
        b_exact
            .cmp(&a_exact)
            .then_with(|| a.username.cmp(&b.username))
    });
    matches.truncate(20);
    Ok(matches)
}

#[cfg(feature = "ssr")]
pub async fn resolve_user_by_id(
    bot_token: &str,
    user_id: &str,
) -> Result<Option<SlackUserResolution>, ServerFnError> {
    let normalized_user_id = user_id.trim();
    if normalized_user_id.is_empty() {
        return Ok(None);
    }

    let client = reqwest::Client::new();
    let mut cursor: Option<String> = None;

    loop {
        let mut query_params = vec![("limit", "200".to_string())];
        if let Some(ref cursor_value) = cursor {
            query_params.push(("cursor", cursor_value.clone()));
        }

        let response = client
            .get("https://slack.com/api/users.list")
            .bearer_auth(bot_token)
            .query(&query_params)
            .send()
            .await
            .map_err(|e| ServerFnError::new(format!("Slack users.list failed: {}", e)))?;
        if !response.status().is_success() {
            return Ok(None);
        }
        let payload = response
            .json::<SlackUsersListResponse>()
            .await
            .map_err(|e| ServerFnError::new(format!("Slack users.list parse failed: {}", e)))?;
        if !payload.ok {
            let error = payload.error.unwrap_or_else(|| "unknown_error".to_string());
            return Err(ServerFnError::new(format!(
                "Slack users.list failed: {error}. Ensure the bot token has users:read scope."
            )));
        }

        if let Some(members) = payload.members {
            for user in members {
                if user.id != normalized_user_id
                    || user.deleted.unwrap_or(false)
                    || user.is_bot.unwrap_or(false)
                {
                    continue;
                }
                let Some(username) = user
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                else {
                    continue;
                };
                let display_name = user
                    .profile
                    .as_ref()
                    .and_then(|profile| profile.display_name.as_deref())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .or_else(|| {
                        user.profile
                            .as_ref()
                            .and_then(|profile| profile.real_name.as_deref())
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                    })
                    .unwrap_or(username.as_str())
                    .to_string();
                let Some(dm_channel_id) = open_dm_channel(bot_token, &user.id).await? else {
                    return Ok(None);
                };
                return Ok(Some(SlackUserResolution {
                    user_id: user.id,
                    username,
                    display_name,
                    dm_channel_id,
                }));
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

#[cfg(feature = "ssr")]
async fn open_dm_channel(bot_token: &str, user_id: &str) -> Result<Option<String>, ServerFnError> {
    let response = reqwest::Client::new()
        .post("https://slack.com/api/conversations.open")
        .bearer_auth(bot_token)
        .form(&[("users", user_id)])
        .send()
        .await
        .map_err(|e| ServerFnError::new(format!("Slack conversations.open failed: {}", e)))?;
    if !response.status().is_success() {
        return Ok(None);
    }
    let payload = response
        .json::<SlackConversationsOpenResponse>()
        .await
        .map_err(|e| ServerFnError::new(format!("Slack conversations.open parse failed: {}", e)))?;
    if !payload.ok {
        return Ok(None);
    }
    Ok(payload.channel.map(|channel| channel.id))
}
