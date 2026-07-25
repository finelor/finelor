use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayIntentKind {
    Help,
}

impl GatewayIntentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Help => "help",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct GatewayIntentArgs {
    pub document_short_ref: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub document_types: Option<Vec<String>>,
    pub confidence_min: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentResolutionSource {
    SlashCommand,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GatewayIntentResolution {
    pub intent: GatewayIntentKind,
    pub confidence: f32,
    pub args: GatewayIntentArgs,
    pub missing_args: Vec<String>,
    pub reason: Option<String>,
    pub source: IntentResolutionSource,
}

pub fn parse_slash_intent(text: &str) -> Option<GatewayIntentResolution> {
    let trimmed = text.trim();
    if !trimmed.starts_with('/') {
        return None;
    }

    let mut parts = trimmed.split_whitespace();
    let command_token = parts.next()?;
    let command = command_token
        .trim_start_matches('/')
        .split('@')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if command != "help" {
        return None;
    }

    Some(GatewayIntentResolution {
        intent: GatewayIntentKind::Help,
        confidence: 1.0,
        args: GatewayIntentArgs::default(),
        missing_args: Vec::new(),
        reason: None,
        source: IntentResolutionSource::SlashCommand,
    })
}

#[derive(Debug, Clone, Copy)]
pub struct IntentDefinition {
    pub name: &'static str,
    pub description: &'static str,
}

pub fn slash_command_menu() -> &'static [IntentDefinition] {
    &[IntentDefinition {
        name: "help",
        description: "Show how to use Finelor",
    }]
}

pub fn normalize_short_ref(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    let digits = trimmed.trim_start_matches(['d', 'D']).trim();
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    let number = digits.parse::<u64>().ok()?;
    Some(format!("D{:06}", number))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_help_slash_command_with_bot_suffix() {
        let parsed = parse_slash_intent("/help@fineloraccountant_bot").unwrap();
        assert_eq!(parsed.intent, GatewayIntentKind::Help);
        assert_eq!(parsed.source, IntentResolutionSource::SlashCommand);
    }

    #[test]
    fn non_help_slash_commands_are_not_gateway_commands() {
        assert!(parse_slash_intent("/start").is_none());
        assert!(parse_slash_intent("/start token").is_none());
        assert!(parse_slash_intent("/status").is_none());
        assert!(parse_slash_intent("/why d12").is_none());
        assert!(parse_slash_intent("/review").is_none());
        assert!(parse_slash_intent("/export").is_none());
    }

    #[test]
    fn slash_command_menu_is_valid_for_telegram() {
        for command in slash_command_menu() {
            assert_ne!(command.name, "start");
            assert!(!command.name.starts_with('/'));
            assert!(command.name.len() <= 32);
            assert!(command.description.len() >= 3);
            assert!(command.description.len() <= 256);
            assert!(
                command
                    .name
                    .chars()
                    .all(|character| character.is_ascii_lowercase()
                        || character.is_ascii_digit()
                        || character == '_')
            );
        }
    }
}
