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
        .json::<serde_json::Value>()
        .await
        .map_err(|e| ServerFnError::new(format!("Slack conversations.info parse failed: {}", e)))?;
    if payload.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Ok(None);
    }
    Ok(payload
        .get("channel")
        .and_then(|channel| channel.get("name"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned))
}
