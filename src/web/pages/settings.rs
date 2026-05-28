use crate::web::client::{copy_to_clipboard, redirect, run_after_ms, subscribe_app_events};
use crate::web::components::ui::{
    ActionButton, Alert, AlertKind, Align, Avatar, Badge, BadgeStyle, Banner, BodyText, Button,
    ButtonKind, ButtonSize, ButtonType, Card, CenterFineText, DesktopSidebar, Drawer,
    DrawerContent, DrawerSide, Eyebrow, FineText, FormField, FormHelp, Grid, GridCols,
    HiddenPlaceholder, IconButton, IconText, InfoItem, Inline, InlinePanelTitle, InlineSurface,
    InlineText, Justify, LinkButton, LoadingCard, Modal, ModalActionGap, ModalActions,
    ModalCloseButton, ModalHeader, ModalInstruction, ModalTitle, PanelTitle, QrCodeBox,
    SETTINGS_DRAWER_ID, SavedBadge, ScrollContent, SettingsDrawerBrand, SettingsDrawerNav,
    SettingsDrawerNavBottom, SettingsDrawerNavTop, SettingsNav, SettingsNavButton,
    SettingsNavDivider, SettingsNavLink, SettingsNavSection, SettingsSubTabButton,
    SettingsSubTabList, SettingsSubTabPanel, SettingsSubTabs, SidebarLayout, Space, Stack,
    TextInput, TextLink, ToastAlert, ToastViewport, Tone,
};
use crate::web::server::api_keys::{
    ApiKeySummary, CreateApiKey, RemoveApiKey, RevokeApiKey, UnrevokeApiKey, list_api_keys,
    reveal_api_key,
};
use crate::web::server::auth::{
    AuthUser, CompanyChannel, CreateTelegramConnectLink, DeleteCompanyChannel, Logout,
    get_session_user, get_telegram_channel_avatar, list_company_channels,
};
use crate::web::server::mcp_keys::{
    CreateMcpKey, McpKeySummary, RemoveMcpKey, RevokeMcpKey, UnrevokeMcpKey, list_mcp_keys,
    reveal_mcp_key,
};
use crate::web::server::settings::{CompanySettings, UpdateCompanySettings, get_company_settings};
use icondata::{
    LuArrowUpRight, LuBot, LuBuilding2, LuCalendar, LuCheck, LuCopy, LuEye, LuFileText, LuLogOut,
    LuMail, LuMessageCircle, LuPlus, LuQrCode, LuRotateCcw, LuScanQrCode, LuSend, LuSettings,
    LuTrash2, LuUser, LuX,
};
use leptos::form::ActionForm;
use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_router::hooks::use_location;
use serde::{Deserialize, Serialize};
use std::{cell::RefCell, rc::Rc};

use crate::web::events::AppEvent;

#[derive(Clone, Debug, Serialize, Deserialize)]
enum SettingsPageState {
    Loaded(SettingsPageData),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SettingsPageData {
    user: AuthUser,
    company: CompanySettings,
    channels: Vec<CompanyChannel>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    ProfileCompany,
    Channels,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChannelSettingsTab {
    Telegram,
    Mcp,
    Api,
}

impl SettingsTab {
    fn from_query_value(tab: Option<String>) -> Self {
        if tab
            .as_deref()
            .is_some_and(|tab| tab.eq_ignore_ascii_case("channels"))
        {
            Self::Channels
        } else {
            Self::ProfileCompany
        }
    }
}

fn telegram_channel_display_name(channel: &CompanyChannel) -> String {
    channel
        .display_name
        .clone()
        .unwrap_or_else(|| format!("Telegram · {}", channel.channel_identifier))
}

fn telegram_channel_username_label(channel: &CompanyChannel) -> Option<String> {
    channel
        .telegram_username
        .as_ref()
        .map(|username| format!("@{}", username))
}

fn telegram_channel_identity_label(channel: &CompanyChannel) -> String {
    match (
        Some(telegram_channel_display_name(channel)),
        telegram_channel_username_label(channel),
    ) {
        (Some(display_name), Some(username)) => format!("{} - {}", display_name, username),
        (Some(display_name), None) => display_name,
        (None, Some(username)) => username,
        (None, None) => "Telegram chat".to_string(),
    }
}

fn format_member_since(created_at: chrono::DateTime<chrono::Utc>) -> String {
    created_at.format("%b %Y").to_string()
}

fn format_api_key_timestamp(value: Option<chrono::DateTime<chrono::Utc>>) -> String {
    value
        .map(|value| value.format("%b %-d, %Y").to_string())
        .unwrap_or_else(|| "Never".to_string())
}

fn format_api_key_created_at(value: chrono::DateTime<chrono::Utc>) -> String {
    value.format("%b %-d, %Y").to_string()
}

#[component]
fn TelegramChannelAvatar(channel_id: i64, label: String) -> impl IntoView {
    let avatar = Resource::new(
        move || channel_id,
        |channel_id| async move {
            get_telegram_channel_avatar(channel_id)
                .await
                .unwrap_or(None)
        },
    );

    view! {
        {move || match avatar.get() {
            Some(Some(src)) => view! { <Avatar src=src alt=label.clone() /> }.into_any(),
            _ => view! {
                <Avatar>
                    <Icon icon=LuMessageCircle width="1.1rem" height="1.1rem" />
                </Avatar>
            }.into_any(),
        }}
    }
}

#[component]
fn McpKeysPanel() -> impl IntoView {
    let refresh_nonce = RwSignal::new(0_u64);
    let create_action = ServerAction::<CreateMcpKey>::new();
    let revoke_action = ServerAction::<RevokeMcpKey>::new();
    let unrevoke_action = ServerAction::<UnrevokeMcpKey>::new();
    let remove_action = ServerAction::<RemoveMcpKey>::new();
    let created_token = RwSignal::<Option<String>>::new(None);
    let copied = RwSignal::new(false);
    let reveal_copied = RwSignal::new(false);
    let create_modal_open = RwSignal::new(false);
    let pending_reveal = RwSignal::<Option<McpKeySummary>>::new(None);
    let pending_revoke = RwSignal::<Option<McpKeySummary>>::new(None);
    let pending_unrevoke = RwSignal::<Option<McpKeySummary>>::new(None);
    let pending_remove = RwSignal::<Option<McpKeySummary>>::new(None);

    let keys = Resource::new(
        move || refresh_nonce.get(),
        |_| async move { list_mcp_keys().await },
    );
    let create_value = create_action.value();
    let revoke_value = revoke_action.value();
    let unrevoke_value = unrevoke_action.value();
    let remove_value = remove_action.value();
    let revealed_token = Resource::new(
        move || pending_reveal.get().map(|key| key.id),
        |mcp_key_id| async move {
            match mcp_key_id {
                Some(mcp_key_id) => reveal_mcp_key(mcp_key_id).await.map(Some),
                None => Ok(None),
            }
        },
    );

    Effect::new(move || {
        if let Some(Ok(created)) = create_value.get() {
            created_token.set(Some(created.token));
            create_modal_open.set(false);
            refresh_nonce.update(|nonce| *nonce += 1);
        }
    });

    Effect::new(move || {
        if let Some(Ok(())) = revoke_value.get() {
            pending_revoke.set(None);
            refresh_nonce.update(|nonce| *nonce += 1);
        }
    });

    Effect::new(move || {
        if let Some(Ok(())) = unrevoke_value.get() {
            pending_unrevoke.set(None);
            refresh_nonce.update(|nonce| *nonce += 1);
        }
    });

    Effect::new(move || {
        if let Some(Ok(())) = remove_value.get() {
            pending_remove.set(None);
            refresh_nonce.update(|nonce| *nonce += 1);
        }
    });

    view! {
        <Stack top=Space::Md gap=Space::Lg>
            <Inline justify=Justify::Between stack_mobile=true>
                <Stack gap=Space::None>
                    <PanelTitle>"MCP keys"</PanelTitle>
                    <FineText>"MCP keys used by AI agents to access Finelor"</FineText>
                    <FineText>
                        <TextLink
                            href="https://github.com/finelor/finelor/blob/main/docs/mcp.md"
                            target="_blank"
                            rel="noopener noreferrer"
                        >
                            "View MCP docs"
                            <Icon icon=LuArrowUpRight width="0.85rem" height="0.85rem" />
                        </TextLink>
                    </FineText>
                </Stack>
                <ActionButton
                    kind=ButtonKind::Primary
                    on_click=move |_| create_modal_open.set(true)
                >
                    <Icon icon=LuPlus width="1rem" height="1rem" />
                    "New MCP Key"
                </ActionButton>
            </Inline>

            <Show when=move || create_value.get().as_ref().and_then(|result| result.as_ref().err()).is_some()>
                <Alert kind=AlertKind::Error>
                    {move || create_value
                        .get()
                        .as_ref()
                        .and_then(|result| result.as_ref().err())
                        .map(|err| format!("Could not create MCP key: {}", err))
                        .unwrap_or_default()}
                </Alert>
            </Show>

            <Show when=move || created_token.get().is_some()>
                <Banner
                    title="MCP key created".to_string()
                    icon=LuCheck
                    tone=Tone::Success
                >
                    <Stack gap=Space::Sm shrink=true>
                        <div class="flex w-full flex-col gap-2 sm:flex-row">
                            <input
                                class="input input-bordered min-w-0 flex-1 font-mono text-sm"
                                readonly
                                prop:value=move || created_token.get().unwrap_or_default()
                            />
                            <ActionButton on_click=move |_| {
                                if let Some(token) = created_token.get() {
                                    copy_to_clipboard(&token);
                                    copied.set(true);
                                    run_after_ms(1_500, move || copied.set(false));
                                }
                            }>
                                <Show
                                    when=move || copied.get()
                                    fallback=move || view! {
                                        <Icon icon=LuCopy width="1rem" height="1rem" />
                                        "Copy"
                                    }
                                >
                                    <Icon icon=LuCheck width="1rem" height="1rem" />
                                    "Copied"
                                </Show>
                            </ActionButton>
                        </div>
                    </Stack>
                </Banner>
            </Show>

            <Show when=move || revoke_value.get().as_ref().and_then(|result| result.as_ref().err()).is_some()>
                <Alert kind=AlertKind::Error>
                    {move || revoke_value
                        .get()
                        .as_ref()
                        .and_then(|result| result.as_ref().err())
                        .map(|err| format!("Could not revoke MCP key: {}", err))
                        .unwrap_or_default()}
                </Alert>
            </Show>

            <Show when=move || unrevoke_value.get().as_ref().and_then(|result| result.as_ref().err()).is_some()>
                <Alert kind=AlertKind::Error>
                    {move || unrevoke_value
                        .get()
                        .as_ref()
                        .and_then(|result| result.as_ref().err())
                        .map(|err| format!("Could not unrevoke MCP key: {}", err))
                        .unwrap_or_default()}
                </Alert>
            </Show>

            <Show when=move || remove_value.get().as_ref().and_then(|result| result.as_ref().err()).is_some()>
                <Alert kind=AlertKind::Error>
                    {move || remove_value
                        .get()
                        .as_ref()
                        .and_then(|result| result.as_ref().err())
                        .map(|err| format!("Could not remove MCP key: {}", err))
                        .unwrap_or_default()}
                </Alert>
            </Show>

            <Suspense fallback=|| view! { <LoadingCard message="Loading MCP keys..." /> }>
                {move || match keys.get() {
                    Some(Ok(keys)) if keys.is_empty() => view! {
                        <Inline align=Align::Start gap=Space::Md surface=InlineSurface::Panel>
                            <Avatar>
                                <Icon icon=LuBot width="1.1rem" height="1.1rem" />
                            </Avatar>
                            <Stack gap=Space::None shrink=true>
                                <PanelTitle>"No MCP keys"</PanelTitle>
                                <FineText>"Create a key before connecting AI agents to Finelor"</FineText>
                            </Stack>
                        </Inline>
                    }.into_any(),
                    Some(Ok(keys)) => view! {
                        <Stack gap=Space::Sm>
                            {keys
                                .into_iter()
                                .map(|key| {
                                    let key_for_revoke = key.clone();
                                    let key_for_unrevoke = key.clone();
                                    let key_for_remove = key.clone();
                                    let key_for_reveal = key.clone();
                                    let revoked = key.revoked_at.is_some();
                                    let key_action = if revoked {
                                        view! {
                                            <Inline gap=Space::Sm>
                                                <IconButton
                                                    label="Unrevoke"
                                                    kind=ButtonKind::Soft
                                                    size=ButtonSize::Sm
                                                    on_click=move |_| pending_unrevoke.set(Some(key_for_unrevoke.clone()))
                                                >
                                                    <Icon icon=LuRotateCcw width="1rem" height="1rem" />
                                                </IconButton>
                                                <IconButton
                                                    label="Remove"
                                                    kind=ButtonKind::SoftDanger
                                                    size=ButtonSize::Sm
                                                    on_click=move |_| pending_remove.set(Some(key_for_remove.clone()))
                                                >
                                                    <Icon icon=LuTrash2 width="1rem" height="1rem" />
                                                </IconButton>
                                            </Inline>
                                        }.into_any()
                                    } else {
                                        view! {
                                            <Inline gap=Space::Sm>
                                                <IconButton
                                                    label="Revoke"
                                                    kind=ButtonKind::SoftDanger
                                                    size=ButtonSize::Sm
                                                    on_click=move |_| pending_revoke.set(Some(key_for_revoke.clone()))
                                                >
                                                    <Icon icon=LuX width="1rem" height="1rem" />
                                                </IconButton>
                                            </Inline>
                                        }.into_any()
                                    };
                                    view! {
                                        <Inline align=Align::Start justify=Justify::Between gap=Space::Md surface=InlineSurface::Panel>
                                            <Stack gap=Space::None shrink=true>
                                                <Inline gap=Space::Sm wrap=true>
                                                    <InlinePanelTitle>{key.name.clone()}</InlinePanelTitle>
                                                    <Badge
                                                        tone=if revoked { Tone::Warning } else { Tone::Success }
                                                        style=BadgeStyle::Soft
                                                    >
                                                        {if revoked { "Revoked" } else { "Active" }}
                                                    </Badge>
                                                </Inline>
                                                <FineText>{format!("Created {} - Last used {}", format_api_key_created_at(key.created_at), format_api_key_timestamp(key.last_used_at))}</FineText>
                                            </Stack>
                                            <Inline gap=Space::Sm>
                                                <IconButton
                                                    label="Reveal"
                                                    kind=ButtonKind::Soft
                                                    size=ButtonSize::Sm
                                                    on_click=move |_| {
                                                        reveal_copied.set(false);
                                                        pending_reveal.set(Some(key_for_reveal.clone()));
                                                    }
                                                >
                                                    <Icon icon=LuEye width="1rem" height="1rem" />
                                                </IconButton>
                                                {key_action}
                                            </Inline>
                                        </Inline>
                                    }
                                })
                                .collect_view()}
                        </Stack>
                    }.into_any(),
                    Some(Err(err)) => view! {
                        <Alert kind=AlertKind::Error>{format!("Failed to load MCP keys: {}", err)}</Alert>
                    }.into_any(),
                    None => view! { <HiddenPlaceholder /> }.into_any(),
                }}
            </Suspense>

            <Show when=move || create_modal_open.get()>
                <Modal close_label="Cancel creating MCP key" on_close=move |_| create_modal_open.set(false)>
                    <ModalCloseButton label="Cancel creating MCP key" on_click=move |_| create_modal_open.set(false)>
                        <Icon icon=LuX width="1.25rem" height="1.25rem" />
                    </ModalCloseButton>
                    <ModalHeader>
                        <Inline gap=Space::Md shrink=true>
                            <Avatar>
                                <Icon icon=LuBot width="1.1rem" height="1.1rem" />
                            </Avatar>
                            <div>
                                <PanelTitle>"New MCP Key"</PanelTitle>
                                <FineText>"AI agent authentication"</FineText>
                            </div>
                        </Inline>
                    </ModalHeader>
                    <ModalTitle>"Create a new MCP key"</ModalTitle>
                    <BodyText>"The generated key can be revealed later from this settings page."</BodyText>
                    <ActionForm action=create_action>
                        <Stack top=Space::Md gap=Space::Md>
                            <FormField label="Key name">
                                <TextInput name="name" placeholder="Claude Desktop" required=true />
                            </FormField>
                            <ModalActions>
                                <ActionButton on_click=move |_| create_modal_open.set(false)>"Cancel"</ActionButton>
                                <Button kind=ButtonKind::Primary button_type=ButtonType::Submit>
                                    <Icon icon=LuPlus width="1rem" height="1rem" />
                                    "Create key"
                                </Button>
                            </ModalActions>
                        </Stack>
                    </ActionForm>
                </Modal>
            </Show>

            <Show when=move || pending_reveal.get().is_some()>
                {move || pending_reveal
                    .get()
                    .map(|key| view! {
                        <Modal close_label="Close MCP key reveal" on_close=move |_| pending_reveal.set(None)>
                            <ModalCloseButton label="Close MCP key reveal" on_click=move |_| pending_reveal.set(None)>
                                <Icon icon=LuX width="1.25rem" height="1.25rem" />
                            </ModalCloseButton>
                            <ModalHeader>
                                <Inline gap=Space::Md shrink=true>
                                    <Avatar><Icon icon=LuEye width="1.1rem" height="1.1rem" /></Avatar>
                                    <div>
                                        <PanelTitle>"Reveal MCP key"</PanelTitle>
                                        <FineText>{key.name.clone()}</FineText>
                                    </div>
                                </Inline>
                            </ModalHeader>
                            <ModalTitle>"MCP key"</ModalTitle>
                            <Suspense fallback=move || view! {
                                <div class="flex items-center gap-3 text-sm text-base-content/70">
                                    <span class="loading loading-spinner loading-sm"></span>
                                    <span>"Loading key..."</span>
                                </div>
                            }>
                                {move || match revealed_token.get() {
                                    Some(Ok(Some(token))) => view! {
                                        <Stack top=Space::Md gap=Space::Md>
                                            <div class="flex w-full flex-col gap-2 sm:flex-row">
                                                <input
                                                    class="input input-bordered min-w-0 flex-1 font-mono text-sm"
                                                    readonly
                                                    prop:value=token.clone()
                                                />
                                                <ActionButton on_click=move |_| {
                                                    copy_to_clipboard(&token);
                                                    reveal_copied.set(true);
                                                    run_after_ms(1_500, move || reveal_copied.set(false));
                                                }>
                                                    <Show
                                                        when=move || reveal_copied.get()
                                                        fallback=move || view! {
                                                            <Icon icon=LuCopy width="1rem" height="1rem" />
                                                            "Copy"
                                                        }
                                                    >
                                                        <Icon icon=LuCheck width="1rem" height="1rem" />
                                                        "Copied"
                                                    </Show>
                                                </ActionButton>
                                            </div>
                                            <ModalActions>
                                                <ActionButton on_click=move |_| pending_reveal.set(None)>"Close"</ActionButton>
                                            </ModalActions>
                                        </Stack>
                                    }.into_any(),
                                    Some(Ok(None)) => view! {
                                        <Alert kind=AlertKind::Error>"MCP key not found."</Alert>
                                    }.into_any(),
                                    Some(Err(err)) => view! {
                                        <Alert kind=AlertKind::Error>{format!("Could not reveal MCP key: {}", err)}</Alert>
                                    }.into_any(),
                                    None => view! { <HiddenPlaceholder /> }.into_any(),
                                }}
                            </Suspense>
                        </Modal>
                    }.into_any())
                    .unwrap_or_else(|| ().into_any())}
            </Show>

            <Show when=move || pending_revoke.get().is_some()>
                {move || pending_revoke
                    .get()
                    .map(|key| view! {
                        <Modal close_label="Cancel revoking MCP key" on_close=move |_| pending_revoke.set(None)>
                            <ModalCloseButton label="Cancel revoking MCP key" on_click=move |_| pending_revoke.set(None)>
                                <Icon icon=LuX width="1.25rem" height="1.25rem" />
                            </ModalCloseButton>
                            <ModalHeader>
                                <Inline gap=Space::Md shrink=true>
                                    <Avatar><Icon icon=LuX width="1.1rem" height="1.1rem" /></Avatar>
                                    <div>
                                        <PanelTitle>"Revoke MCP key"</PanelTitle>
                                        <FineText>{key.name.clone()}</FineText>
                                    </div>
                                </Inline>
                            </ModalHeader>
                            <ModalTitle>"Revoke this key?"</ModalTitle>
                            <BodyText>"AI agents using this key will immediately lose MCP access."</BodyText>
                            <ActionForm action=revoke_action>
                                <input type="hidden" name="mcp_key_id" value=key.id.to_string() />
                                <ModalActions>
                                    <ActionButton on_click=move |_| pending_revoke.set(None)>"Cancel"</ActionButton>
                                    <Button kind=ButtonKind::SoftDanger button_type=ButtonType::Submit>
                                        <Icon icon=LuX width="1rem" height="1rem" />
                                        "Revoke"
                                    </Button>
                                </ModalActions>
                            </ActionForm>
                        </Modal>
                    }.into_any())
                    .unwrap_or_else(|| ().into_any())}
            </Show>

            <Show when=move || pending_unrevoke.get().is_some()>
                {move || pending_unrevoke
                    .get()
                    .map(|key| view! {
                        <Modal close_label="Cancel unrevoking MCP key" on_close=move |_| pending_unrevoke.set(None)>
                            <ModalCloseButton label="Cancel unrevoking MCP key" on_click=move |_| pending_unrevoke.set(None)>
                                <Icon icon=LuX width="1.25rem" height="1.25rem" />
                            </ModalCloseButton>
                            <ModalHeader>
                                <Inline gap=Space::Md shrink=true>
                                    <Avatar><Icon icon=LuRotateCcw width="1.1rem" height="1.1rem" /></Avatar>
                                    <div>
                                        <PanelTitle>"Unrevoke MCP key"</PanelTitle>
                                        <FineText>{key.name.clone()}</FineText>
                                    </div>
                                </Inline>
                            </ModalHeader>
                            <ModalTitle>"Restore this key?"</ModalTitle>
                            <BodyText>"AI agents using this key will be able to access MCP again."</BodyText>
                            <ActionForm action=unrevoke_action>
                                <input type="hidden" name="mcp_key_id" value=key.id.to_string() />
                                <ModalActions>
                                    <ActionButton on_click=move |_| pending_unrevoke.set(None)>"Cancel"</ActionButton>
                                    <Button kind=ButtonKind::Primary button_type=ButtonType::Submit>
                                        <Icon icon=LuRotateCcw width="1rem" height="1rem" />
                                        "Unrevoke"
                                    </Button>
                                </ModalActions>
                            </ActionForm>
                        </Modal>
                    }.into_any())
                    .unwrap_or_else(|| ().into_any())}
            </Show>

            <Show when=move || pending_remove.get().is_some()>
                {move || pending_remove
                    .get()
                    .map(|key| view! {
                        <Modal close_label="Cancel removing MCP key" on_close=move |_| pending_remove.set(None)>
                            <ModalCloseButton label="Cancel removing MCP key" on_click=move |_| pending_remove.set(None)>
                                <Icon icon=LuX width="1.25rem" height="1.25rem" />
                            </ModalCloseButton>
                            <ModalHeader>
                                <Inline gap=Space::Md shrink=true>
                                    <Avatar><Icon icon=LuTrash2 width="1.1rem" height="1.1rem" /></Avatar>
                                    <div>
                                        <PanelTitle>"Remove MCP key"</PanelTitle>
                                        <FineText>{key.name.clone()}</FineText>
                                    </div>
                                </Inline>
                            </ModalHeader>
                            <ModalTitle>"Remove this revoked key?"</ModalTitle>
                            <BodyText>"The key stays revoked and will no longer appear in this list."</BodyText>
                            <ActionForm action=remove_action>
                                <input type="hidden" name="mcp_key_id" value=key.id.to_string() />
                                <ModalActions>
                                    <ActionButton on_click=move |_| pending_remove.set(None)>"Cancel"</ActionButton>
                                    <Button kind=ButtonKind::SoftDanger button_type=ButtonType::Submit>
                                        <Icon icon=LuTrash2 width="1rem" height="1rem" />
                                        "Remove"
                                    </Button>
                                </ModalActions>
                            </ActionForm>
                        </Modal>
                    }.into_any())
                    .unwrap_or_else(|| ().into_any())}
            </Show>
        </Stack>
    }
}

#[component]
fn ApiKeysPanel() -> impl IntoView {
    let refresh_nonce = RwSignal::new(0_u64);
    let create_action = ServerAction::<CreateApiKey>::new();
    let revoke_action = ServerAction::<RevokeApiKey>::new();
    let unrevoke_action = ServerAction::<UnrevokeApiKey>::new();
    let remove_action = ServerAction::<RemoveApiKey>::new();
    let created_token = RwSignal::<Option<String>>::new(None);
    let copied = RwSignal::new(false);
    let reveal_copied = RwSignal::new(false);
    let create_modal_open = RwSignal::new(false);
    let pending_reveal = RwSignal::<Option<ApiKeySummary>>::new(None);
    let pending_revoke = RwSignal::<Option<ApiKeySummary>>::new(None);
    let pending_unrevoke = RwSignal::<Option<ApiKeySummary>>::new(None);
    let pending_remove = RwSignal::<Option<ApiKeySummary>>::new(None);

    let keys = Resource::new(
        move || refresh_nonce.get(),
        |_| async move { list_api_keys().await },
    );
    let create_value = create_action.value();
    let revoke_value = revoke_action.value();
    let unrevoke_value = unrevoke_action.value();
    let remove_value = remove_action.value();
    let revealed_token = Resource::new(
        move || pending_reveal.get().map(|key| key.id),
        |api_key_id| async move {
            match api_key_id {
                Some(api_key_id) => reveal_api_key(api_key_id).await.map(Some),
                None => Ok(None),
            }
        },
    );

    Effect::new(move || {
        if let Some(Ok(created)) = create_value.get() {
            created_token.set(Some(created.token));
            create_modal_open.set(false);
            refresh_nonce.update(|nonce| *nonce += 1);
        }
    });

    Effect::new(move || {
        if let Some(Ok(())) = revoke_value.get() {
            pending_revoke.set(None);
            refresh_nonce.update(|nonce| *nonce += 1);
        }
    });

    Effect::new(move || {
        if let Some(Ok(())) = unrevoke_value.get() {
            pending_unrevoke.set(None);
            refresh_nonce.update(|nonce| *nonce += 1);
        }
    });

    Effect::new(move || {
        if let Some(Ok(())) = remove_value.get() {
            pending_remove.set(None);
            refresh_nonce.update(|nonce| *nonce += 1);
        }
    });

    view! {
        <Stack top=Space::Md gap=Space::Lg>
            <Inline justify=Justify::Between stack_mobile=true>
                <Stack gap=Space::None>
                    <PanelTitle>"API keys"</PanelTitle>
                    <FineText>"API keys used for authentication to the public API"</FineText>
                    <FineText>
                        <TextLink
                            href="https://github.com/finelor/finelor/blob/main/docs/api.md"
                            target="_blank"
                            rel="noopener noreferrer"
                        >
                            "View API docs"
                            <Icon icon=LuArrowUpRight width="0.85rem" height="0.85rem" />
                        </TextLink>
                    </FineText>
                </Stack>
                <ActionButton
                    kind=ButtonKind::Primary
                    on_click=move |_| create_modal_open.set(true)
                >
                    <Icon icon=LuPlus width="1rem" height="1rem" />
                    "New API Key"
                </ActionButton>
            </Inline>

            <Show when=move || create_value.get().as_ref().and_then(|result| result.as_ref().err()).is_some()>
                <Alert kind=AlertKind::Error>
                    {move || create_value
                        .get()
                        .as_ref()
                        .and_then(|result| result.as_ref().err())
                        .map(|err| format!("Could not create API key: {}", err))
                        .unwrap_or_default()}
                </Alert>
            </Show>

            <Show when=move || created_token.get().is_some()>
                <Banner
                    title="API key created".to_string()
                    icon=LuCheck
                    tone=Tone::Success
                >
                    <Stack gap=Space::Sm shrink=true>
                        <div class="flex w-full flex-col gap-2 sm:flex-row">
                            <input
                                class="input input-bordered min-w-0 flex-1 font-mono text-sm"
                                readonly
                                prop:value=move || created_token.get().unwrap_or_default()
                            />
                            <ActionButton on_click=move |_| {
                                if let Some(token) = created_token.get() {
                                    copy_to_clipboard(&token);
                                    copied.set(true);
                                    run_after_ms(1_500, move || copied.set(false));
                                }
                            }>
                                <Show
                                    when=move || copied.get()
                                    fallback=move || view! {
                                        <Icon icon=LuCopy width="1rem" height="1rem" />
                                        "Copy"
                                    }
                                >
                                    <Icon icon=LuCheck width="1rem" height="1rem" />
                                    "Copied"
                                </Show>
                            </ActionButton>
                        </div>
                    </Stack>
                </Banner>
            </Show>

            <Show when=move || revoke_value.get().as_ref().and_then(|result| result.as_ref().err()).is_some()>
                <Alert kind=AlertKind::Error>
                    {move || revoke_value
                        .get()
                        .as_ref()
                        .and_then(|result| result.as_ref().err())
                        .map(|err| format!("Could not revoke API key: {}", err))
                        .unwrap_or_default()}
                </Alert>
            </Show>

            <Show when=move || unrevoke_value.get().as_ref().and_then(|result| result.as_ref().err()).is_some()>
                <Alert kind=AlertKind::Error>
                    {move || unrevoke_value
                        .get()
                        .as_ref()
                        .and_then(|result| result.as_ref().err())
                        .map(|err| format!("Could not unrevoke API key: {}", err))
                        .unwrap_or_default()}
                </Alert>
            </Show>

            <Show when=move || remove_value.get().as_ref().and_then(|result| result.as_ref().err()).is_some()>
                <Alert kind=AlertKind::Error>
                    {move || remove_value
                        .get()
                        .as_ref()
                        .and_then(|result| result.as_ref().err())
                        .map(|err| format!("Could not remove API key: {}", err))
                        .unwrap_or_default()}
                </Alert>
            </Show>

            <Suspense fallback=|| view! { <LoadingCard message="Loading API keys..." /> }>
                {move || match keys.get() {
                    Some(Ok(keys)) if keys.is_empty() => view! {
                        <Inline align=Align::Start gap=Space::Md surface=InlineSurface::Panel>
                            <Avatar>
                                <Icon icon=LuFileText width="1.1rem" height="1.1rem" />
                            </Avatar>
                            <Stack gap=Space::None shrink=true>
                                <PanelTitle>"No API keys"</PanelTitle>
                                <FineText>"Create a key before connecting systems to Finelor"</FineText>
                            </Stack>
                        </Inline>
                    }.into_any(),
                    Some(Ok(keys)) => view! {
                        <Stack gap=Space::Sm>
                            {keys
                                .into_iter()
                                .map(|key| {
                                    let key_for_revoke = key.clone();
                                    let key_for_unrevoke = key.clone();
                                    let key_for_remove = key.clone();
                                    let key_for_reveal = key.clone();
                                    let revoked = key.revoked_at.is_some();
                                    let key_action = if revoked {
                                        view! {
                                            <Inline gap=Space::Sm>
                                                <IconButton
                                                    label="Unrevoke"
                                                    kind=ButtonKind::Soft
                                                    size=ButtonSize::Sm
                                                    on_click=move |_| pending_unrevoke.set(Some(key_for_unrevoke.clone()))
                                                >
                                                    <Icon icon=LuRotateCcw width="1rem" height="1rem" />
                                                </IconButton>
                                                <IconButton
                                                    label="Remove"
                                                    kind=ButtonKind::SoftDanger
                                                    size=ButtonSize::Sm
                                                    on_click=move |_| pending_remove.set(Some(key_for_remove.clone()))
                                                >
                                                    <Icon icon=LuTrash2 width="1rem" height="1rem" />
                                                </IconButton>
                                            </Inline>
                                        }.into_any()
                                    } else {
                                        view! {
                                            <Inline gap=Space::Sm>
                                                <IconButton
                                                    label="Revoke"
                                                    kind=ButtonKind::SoftDanger
                                                    size=ButtonSize::Sm
                                                    on_click=move |_| pending_revoke.set(Some(key_for_revoke.clone()))
                                                >
                                                    <Icon icon=LuX width="1rem" height="1rem" />
                                                </IconButton>
                                            </Inline>
                                        }.into_any()
                                    };
                                    view! {
                                        <Inline align=Align::Start justify=Justify::Between gap=Space::Md surface=InlineSurface::Panel>
                                            <Stack gap=Space::None shrink=true>
                                                <Inline gap=Space::Sm wrap=true>
                                                    <InlinePanelTitle>{key.name.clone()}</InlinePanelTitle>
                                                    <Badge
                                                        tone=if revoked { Tone::Warning } else { Tone::Success }
                                                        style=BadgeStyle::Soft
                                                    >
                                                        {if revoked { "Revoked" } else { "Active" }}
                                                    </Badge>
                                                </Inline>
                                                <FineText>{format!("Created {} - Last used {}", format_api_key_created_at(key.created_at), format_api_key_timestamp(key.last_used_at))}</FineText>
                                            </Stack>
                                            <Inline gap=Space::Sm>
                                                <IconButton
                                                    label="Reveal"
                                                    kind=ButtonKind::Soft
                                                    size=ButtonSize::Sm
                                                    on_click=move |_| {
                                                        reveal_copied.set(false);
                                                        pending_reveal.set(Some(key_for_reveal.clone()));
                                                    }
                                                >
                                                    <Icon icon=LuEye width="1rem" height="1rem" />
                                                </IconButton>
                                                {key_action}
                                            </Inline>
                                        </Inline>
                                    }
                                })
                                .collect_view()}
                        </Stack>
                    }.into_any(),
                    Some(Err(err)) => view! {
                        <Alert kind=AlertKind::Error>{format!("Failed to load API keys: {}", err)}</Alert>
                    }.into_any(),
                    None => view! { <HiddenPlaceholder /> }.into_any(),
                }}
            </Suspense>

            <Show when=move || create_modal_open.get()>
                <Modal close_label="Cancel creating API key" on_close=move |_| create_modal_open.set(false)>
                    <ModalCloseButton label="Cancel creating API key" on_click=move |_| create_modal_open.set(false)>
                        <Icon icon=LuX width="1.25rem" height="1.25rem" />
                    </ModalCloseButton>
                    <ModalHeader>
                        <Inline gap=Space::Md shrink=true>
                            <Avatar>
                                <Icon icon=LuFileText width="1.1rem" height="1.1rem" />
                            </Avatar>
                            <div>
                                <PanelTitle>"New API Key"</PanelTitle>
                                <FineText>"Public API authentication"</FineText>
                            </div>
                        </Inline>
                    </ModalHeader>
                    <ModalTitle>"Create a new API key"</ModalTitle>
                    <BodyText>"The generated key will only be shown once."</BodyText>
                    <ActionForm action=create_action>
                        <Stack top=Space::Md gap=Space::Md>
                            <FormField label="Key name">
                                <TextInput name="name" placeholder="Production integration" required=true />
                            </FormField>
                            <ModalActions>
                                <ActionButton on_click=move |_| create_modal_open.set(false)>"Cancel"</ActionButton>
                                <Button kind=ButtonKind::Primary button_type=ButtonType::Submit>
                                    <Icon icon=LuPlus width="1rem" height="1rem" />
                                    "Create key"
                                </Button>
                            </ModalActions>
                        </Stack>
                    </ActionForm>
                </Modal>
            </Show>

            <Show when=move || pending_reveal.get().is_some()>
                {move || pending_reveal
                    .get()
                    .map(|key| view! {
                        <Modal close_label="Close API key reveal" on_close=move |_| pending_reveal.set(None)>
                            <ModalCloseButton label="Close API key reveal" on_click=move |_| pending_reveal.set(None)>
                                <Icon icon=LuX width="1.25rem" height="1.25rem" />
                            </ModalCloseButton>
                            <ModalHeader>
                                <Inline gap=Space::Md shrink=true>
                                    <Avatar><Icon icon=LuEye width="1.1rem" height="1.1rem" /></Avatar>
                                    <div>
                                        <PanelTitle>"Reveal API key"</PanelTitle>
                                        <FineText>{key.name.clone()}</FineText>
                                    </div>
                                </Inline>
                            </ModalHeader>
                            <ModalTitle>"API key"</ModalTitle>
                            <Suspense fallback=move || view! {
                                <div class="flex items-center gap-3 text-sm text-base-content/70">
                                    <span class="loading loading-spinner loading-sm"></span>
                                    <span>"Loading key..."</span>
                                </div>
                            }>
                                {move || match revealed_token.get() {
                                    Some(Ok(Some(token))) => view! {
                                        <Stack top=Space::Md gap=Space::Md>
                                            <div class="flex w-full flex-col gap-2 sm:flex-row">
                                                <input
                                                    class="input input-bordered min-w-0 flex-1 font-mono text-sm"
                                                    readonly
                                                    prop:value=token.clone()
                                                />
                                                <ActionButton on_click=move |_| {
                                                    copy_to_clipboard(&token);
                                                    reveal_copied.set(true);
                                                    run_after_ms(1_500, move || reveal_copied.set(false));
                                                }>
                                                    <Show
                                                        when=move || reveal_copied.get()
                                                        fallback=move || view! {
                                                            <Icon icon=LuCopy width="1rem" height="1rem" />
                                                            "Copy"
                                                        }
                                                    >
                                                        <Icon icon=LuCheck width="1rem" height="1rem" />
                                                        "Copied"
                                                    </Show>
                                                </ActionButton>
                                            </div>
                                            <ModalActions>
                                                <ActionButton on_click=move |_| pending_reveal.set(None)>"Close"</ActionButton>
                                            </ModalActions>
                                        </Stack>
                                    }.into_any(),
                                    Some(Ok(None)) => view! {
                                        <Alert kind=AlertKind::Error>"API key not found."</Alert>
                                    }.into_any(),
                                    Some(Err(err)) => view! {
                                        <Alert kind=AlertKind::Error>{format!("Could not reveal API key: {}", err)}</Alert>
                                    }.into_any(),
                                    None => view! { <HiddenPlaceholder /> }.into_any(),
                                }}
                            </Suspense>
                        </Modal>
                    }.into_any())
                    .unwrap_or_else(|| ().into_any())}
            </Show>

            <Show when=move || pending_revoke.get().is_some()>
                {move || pending_revoke
                    .get()
                    .map(|key| view! {
                        <Modal close_label="Cancel revoking API key" on_close=move |_| pending_revoke.set(None)>
                            <ModalCloseButton label="Cancel revoking API key" on_click=move |_| pending_revoke.set(None)>
                                <Icon icon=LuX width="1.25rem" height="1.25rem" />
                            </ModalCloseButton>
                            <ModalHeader>
                                <Inline gap=Space::Md shrink=true>
                                    <Avatar><Icon icon=LuX width="1.1rem" height="1.1rem" /></Avatar>
                                    <div>
                                        <PanelTitle>"Revoke API key"</PanelTitle>
                                        <FineText>{key.name.clone()}</FineText>
                                    </div>
                                </Inline>
                            </ModalHeader>
                            <ModalTitle>"Revoke this key?"</ModalTitle>
                            <BodyText>"External systems using this key will immediately lose access."</BodyText>
                            <ActionForm action=revoke_action>
                                <input type="hidden" name="api_key_id" value=key.id.to_string() />
                                <ModalActions>
                                    <ActionButton on_click=move |_| pending_revoke.set(None)>"Cancel"</ActionButton>
                                    <Button kind=ButtonKind::SoftDanger button_type=ButtonType::Submit>
                                        <Icon icon=LuX width="1rem" height="1rem" />
                                        "Revoke"
                                    </Button>
                                </ModalActions>
                            </ActionForm>
                        </Modal>
                    }.into_any())
                    .unwrap_or_else(|| ().into_any())}
            </Show>

            <Show when=move || pending_unrevoke.get().is_some()>
                {move || pending_unrevoke
                    .get()
                    .map(|key| view! {
                        <Modal close_label="Cancel unrevoking API key" on_close=move |_| pending_unrevoke.set(None)>
                            <ModalCloseButton label="Cancel unrevoking API key" on_click=move |_| pending_unrevoke.set(None)>
                                <Icon icon=LuX width="1.25rem" height="1.25rem" />
                            </ModalCloseButton>
                            <ModalHeader>
                                <Inline gap=Space::Md shrink=true>
                                    <Avatar><Icon icon=LuRotateCcw width="1.1rem" height="1.1rem" /></Avatar>
                                    <div>
                                        <PanelTitle>"Unrevoke API key"</PanelTitle>
                                        <FineText>{key.name.clone()}</FineText>
                                    </div>
                                </Inline>
                            </ModalHeader>
                            <ModalTitle>"Restore this key?"</ModalTitle>
                            <BodyText>"External systems using this key will be able to access the public API again."</BodyText>
                            <ActionForm action=unrevoke_action>
                                <input type="hidden" name="api_key_id" value=key.id.to_string() />
                                <ModalActions>
                                    <ActionButton on_click=move |_| pending_unrevoke.set(None)>"Cancel"</ActionButton>
                                    <Button kind=ButtonKind::Primary button_type=ButtonType::Submit>
                                        <Icon icon=LuRotateCcw width="1rem" height="1rem" />
                                        "Unrevoke"
                                    </Button>
                                </ModalActions>
                            </ActionForm>
                        </Modal>
                    }.into_any())
                    .unwrap_or_else(|| ().into_any())}
            </Show>

            <Show when=move || pending_remove.get().is_some()>
                {move || pending_remove
                    .get()
                    .map(|key| view! {
                        <Modal close_label="Cancel removing API key" on_close=move |_| pending_remove.set(None)>
                            <ModalCloseButton label="Cancel removing API key" on_click=move |_| pending_remove.set(None)>
                                <Icon icon=LuX width="1.25rem" height="1.25rem" />
                            </ModalCloseButton>
                            <ModalHeader>
                                <Inline gap=Space::Md shrink=true>
                                    <Avatar><Icon icon=LuTrash2 width="1.1rem" height="1.1rem" /></Avatar>
                                    <div>
                                        <PanelTitle>"Remove API key"</PanelTitle>
                                        <FineText>{key.name.clone()}</FineText>
                                    </div>
                                </Inline>
                            </ModalHeader>
                            <ModalTitle>"Remove this revoked key?"</ModalTitle>
                            <BodyText>"The key stays revoked and will no longer appear in this list."</BodyText>
                            <ActionForm action=remove_action>
                                <input type="hidden" name="api_key_id" value=key.id.to_string() />
                                <ModalActions>
                                    <ActionButton on_click=move |_| pending_remove.set(None)>"Cancel"</ActionButton>
                                    <Button kind=ButtonKind::SoftDanger button_type=ButtonType::Submit>
                                        <Icon icon=LuTrash2 width="1rem" height="1rem" />
                                        "Remove"
                                    </Button>
                                </ModalActions>
                            </ActionForm>
                        </Modal>
                    }.into_any())
                    .unwrap_or_else(|| ().into_any())}
            </Show>
        </Stack>
    }
}

#[component]
pub fn Settings() -> impl IntoView {
    let update_action = ServerAction::<UpdateCompanySettings>::new();
    let logout_action = ServerAction::<Logout>::new();
    let telegram_link_action = ServerAction::<CreateTelegramConnectLink>::new();
    let delete_channel_action = ServerAction::<DeleteCompanyChannel>::new();
    let telegram_modal_link = RwSignal::new(None);
    let channel_pending_delete = RwSignal::<Option<CompanyChannel>>::new(None);
    let telegram_connect_error = RwSignal::<Option<String>>::new(None);
    let channel_notice = RwSignal::<Option<String>>::new(None);
    let live_updates_disconnected = RwSignal::new(false);
    let refresh_nonce = RwSignal::new(0_u64);
    let location = use_location();
    let active_tab = RwSignal::new(SettingsTab::from_query_value(
        location.query.get_untracked().get("tab"),
    ));
    let active_channel_tab = RwSignal::new(ChannelSettingsTab::Telegram);

    Effect::new(move || {
        active_tab.set(SettingsTab::from_query_value(
            location.query.get().get("tab"),
        ));
    });

    let page_data = Resource::new(
        move || refresh_nonce.get(),
        |_| async move {
            let user = get_session_user()
                .await?
                .ok_or_else(|| ServerFnError::new("Not authenticated."))?;
            Ok::<_, ServerFnError>(SettingsPageState::Loaded(SettingsPageData {
                user,
                company: get_company_settings().await?,
                channels: list_company_channels().await?,
            }))
        },
    );

    let update_value = update_action.value();
    let logout_value = logout_action.value();
    let telegram_link_value = telegram_link_action.value();
    let delete_channel_value = delete_channel_action.value();

    Effect::new(move || {
        if let Some(Ok(())) = update_value.get() {
            refresh_nonce.update(|nonce| *nonce += 1);
        }
    });

    Effect::new(move || {
        if let Some(Ok(())) = logout_value.get() {
            redirect("/login");
        }
    });

    Effect::new(move || {
        if let Some(Ok(link)) = telegram_link_value.get() {
            telegram_connect_error.set(None);
            telegram_modal_link.set(Some(link));
        }
    });

    Effect::new(move || {
        if let Some(Ok(())) = delete_channel_value.get() {
            channel_pending_delete.set(None);
            refresh_nonce.update(|nonce| *nonce += 1);
        }
    });

    let event_subscription = Rc::new(RefCell::new(None));
    Effect::new({
        let event_subscription = Rc::clone(&event_subscription);
        move || {
            if event_subscription.borrow().is_some() {
                return;
            }

            let subscription = subscribe_app_events(
                move |data| {
                    live_updates_disconnected.set(false);
                    let Ok(event) = serde_json::from_str::<AppEvent>(&data) else {
                        return;
                    };

                    match event.event_type.as_str() {
                        "sync" | "channels.changed" => {
                            refresh_nonce.update(|nonce| *nonce += 1);
                        }
                        "telegram_connect.connected" => {
                            let event_connect_id = event
                                .payload
                                .get("connect_id")
                                .and_then(|value| value.as_str())
                                .map(ToOwned::to_owned);

                            if let (Some(link), Some(event_connect_id)) =
                                (telegram_modal_link.get_untracked(), event_connect_id)
                                && link.connect_id == event_connect_id
                            {
                                telegram_modal_link.set(None);
                                telegram_connect_error.set(None);
                                channel_notice
                                    .set(Some("Telegram connection established.".to_string()));
                                run_after_ms(6_000, move || channel_notice.set(None));
                            }

                            refresh_nonce.update(|nonce| *nonce += 1);
                        }
                        "telegram_connect.failed" => {
                            let event_connect_id = event
                                .payload
                                .get("connect_id")
                                .and_then(|value| value.as_str())
                                .map(ToOwned::to_owned);

                            if let (Some(link), Some(event_connect_id)) =
                                (telegram_modal_link.get_untracked(), event_connect_id)
                                && link.connect_id == event_connect_id
                            {
                                telegram_connect_error.set(Some(
                                    "Telegram connection failed. Generate a new link and try again."
                                        .to_string(),
                                ));
                            }
                        }
                        _ => {}
                    }
                },
                move || live_updates_disconnected.set(false),
                move || live_updates_disconnected.set(true),
            );

            *event_subscription.borrow_mut() = subscription;
        }
    });

    view! {
        <div>
            <Suspense fallback=|| view! { <LoadingCard message="Loading settings..." /> }>
                {move || match page_data.get() {
                    Some(Ok(SettingsPageState::Loaded(settings))) => {
                        let profile_name = settings
                            .user
                            .display_name
                            .clone()
                            .unwrap_or_else(|| "-".to_string());
                        let profile_email = settings.user.email.clone();
                        let profile_member_since = format_member_since(settings.user.created_at);
                        let profile_name = StoredValue::new(profile_name);
                        let profile_email = StoredValue::new(profile_email);
                        let profile_member_since = StoredValue::new(profile_member_since);
                        let company_name = RwSignal::new(settings.company.display_name.clone());
                        let org_number = RwSignal::new(settings.company.org_nr.clone().unwrap_or_default());
                        let jurisdiction = RwSignal::new(settings.company.jurisdiction.clone());
                        let active_telegram_channels = settings
                            .channels
                            .iter()
                            .filter(|channel| channel.active && channel.channel_type == "TELEGRAM")
                            .cloned()
                            .collect::<Vec<_>>();
                        let has_active_telegram_channels = !active_telegram_channels.is_empty();
                        let telegram_channel_count = active_telegram_channels.len();
                        let active_telegram_channels = RwSignal::new(active_telegram_channels);
                        view! {
                            <>
                                <Drawer drawer_id=SETTINGS_DRAWER_ID>
                                    <DrawerContent>
                                        <SidebarLayout>
                                            <DesktopSidebar>
                                                <SettingsNav>
                                                    <SettingsNavSection>
                                                        <SettingsNavLink
                                                            href="/settings?tab=profile"
                                                            active=move || active_tab.get() == SettingsTab::ProfileCompany
                                                        >
                                                            <Icon icon=LuBuilding2 width="1rem" height="1rem" />
                                                            "Profile & company"
                                                        </SettingsNavLink>
                                                        <SettingsNavLink
                                                            href="/settings?tab=channels"
                                                            active=move || active_tab.get() == SettingsTab::Channels
                                                        >
                                                            <Icon icon=LuSend width="1rem" height="1rem" />
                                                            "Channels"
                                                        </SettingsNavLink>
                                                    </SettingsNavSection>
                                                    <SettingsNavDivider />
                                                    <SettingsNavSection>
                                                        <SettingsNavLink href="/dashboard" active=|| false>
                                                            <Icon icon=LuSettings width="1rem" height="1rem" />
                                                            "Go to dashboard"
                                                        </SettingsNavLink>
                                                        <SettingsNavButton form="settings-logout-form">
                                                            <Icon icon=LuLogOut width="1rem" height="1rem" />
                                                            "Logout"
                                                        </SettingsNavButton>
                                                    </SettingsNavSection>
                                                    <ActionForm action=logout_action attr:id="settings-logout-form">
                                                        <input type="hidden" name="_logout" value="1" />
                                                    </ActionForm>
                                                </SettingsNav>
                                            </DesktopSidebar>

                                            <ScrollContent>
                                                <Show
                                                    when=move || active_tab.get() == SettingsTab::ProfileCompany
                                                    fallback=move || view! {
                                                        <Card>
                                                            <Eyebrow>"Channels"</Eyebrow>
                                                            <BodyText>"Connect channels where you can easily interact with Finelor"</BodyText>

                                                            <SettingsSubTabs>
                                                                <SettingsSubTabList>
                                                                    <SettingsSubTabButton
                                                                        active=move || active_channel_tab.get() == ChannelSettingsTab::Telegram
                                                                        on_click=move |_| active_channel_tab.set(ChannelSettingsTab::Telegram)
                                                                    >
                                                                        <Icon icon=LuSend width="1rem" height="1rem" />
                                                                        "Telegram"
                                                                    </SettingsSubTabButton>
                                                                    <SettingsSubTabButton
                                                                        active=move || active_channel_tab.get() == ChannelSettingsTab::Mcp
                                                                        on_click=move |_| active_channel_tab.set(ChannelSettingsTab::Mcp)
                                                                    >
                                                                        <Icon icon=LuBot width="1rem" height="1rem" />
                                                                        "MCP"
                                                                    </SettingsSubTabButton>
                                                                    <SettingsSubTabButton
                                                                        active=move || active_channel_tab.get() == ChannelSettingsTab::Api
                                                                        on_click=move |_| active_channel_tab.set(ChannelSettingsTab::Api)
                                                                    >
                                                                        <Icon icon=LuFileText width="1rem" height="1rem" />
                                                                        "API"
                                                                    </SettingsSubTabButton>
                                                                </SettingsSubTabList>

                                                                <SettingsSubTabPanel>
                                                                    <Show
                                                                        when=move || active_channel_tab.get() == ChannelSettingsTab::Telegram
                                                                        fallback=move || view! {
                                                                            <Show
                                                                                when=move || active_channel_tab.get() == ChannelSettingsTab::Mcp
                                                                                fallback=move || view! { <ApiKeysPanel /> }
                                                                            >
                                                                                <McpKeysPanel />
                                                                            </Show>
                                                                        }
                                                                    >
                                                                        <Stack top=Space::Md gap=Space::Md>
                                                                            <Inline justify=Justify::Between stack_mobile=true>
                                                                                <Stack gap=Space::None>
                                                                                    <PanelTitle>"Telegram connections"</PanelTitle>
                                                                                    <FineText>
                                                                                        {if has_active_telegram_channels {
                                                                                            format!("{} active connection{}", telegram_channel_count, if telegram_channel_count == 1 { "" } else { "s" })
                                                                                        } else {
                                                                                            "No Telegram connections yet".to_string()
                                                                                        }}
                                                                                    </FineText>
                                                                                </Stack>
                                                                                <ActionButton
                                                                                    kind=ButtonKind::Primary
                                                                                    on_click=move |_| {
                                                                                        telegram_link_action.dispatch(CreateTelegramConnectLink {});
                                                                                    }
                                                                                >
                                                                                        <Icon icon=LuPlus width="1rem" height="1rem" />
                                                                                        "New connection"
                                                                                </ActionButton>
                                                                            </Inline>

                                                                            <Show when=move || live_updates_disconnected.get()>
                                                                                <Alert kind=AlertKind::Warning>
                                                                                    "Live updates disconnected. This page will keep checking while the Telegram connection dialog is open."
                                                                                </Alert>
                                                                            </Show>

                                                                                <Show
                                                                                    when=move || has_active_telegram_channels
                                                                                    fallback=move || view! {
                                                                                        <Inline align=Align::Start gap=Space::Md surface=InlineSurface::Panel>
                                                                                            <Avatar>
                                                                                                <Icon icon=LuSend width="1.1rem" height="1.1rem" />
                                                                                            </Avatar>
                                                                                            <Stack gap=Space::None shrink=true>
                                                                                                <PanelTitle>"Connect Telegram"</PanelTitle>
                                                                                                <FineText>"Add a Telegram chat so Finelor can receive documents and send updates."</FineText>
                                                                                            </Stack>
                                                                                        </Inline>
                                                                                    }
                                                                                >
                                                                                <Stack gap=Space::Sm>
                                                                                    {move || active_telegram_channels
                                                                                            .get()
                                                                                            .into_iter()
                                                                                            .map(|channel| {
                                                                                            let channel_for_remove = channel.clone();
                                                                                            let display_name = telegram_channel_display_name(&channel);
                                                                                            let chat_type = channel.telegram_chat_type.clone().map(|value| match value.as_str() {
                                                                                                "private" => "Private chat".to_string(),
                                                                                                "group" => "Group".to_string(),
                                                                                                "supergroup" => "Supergroup".to_string(),
                                                                                                "channel" => "Channel".to_string(),
                                                                                                _ => value,
                                                                                            });
                                                                                            let secondary = match (channel.telegram_username.clone(), chat_type) {
                                                                                                (Some(username), Some(chat_type)) => format!("{} - @{}", chat_type, username),
                                                                                                (Some(username), None) => format!("@{}", username),
                                                                                                (None, Some(chat_type)) => chat_type,
                                                                                                (None, None) => "Telegram chat".to_string(),
                                                                                            };
                                                                                            let avatar_label = display_name.clone();
                                                                                            view! {
                                                                                                <Inline align=Align::Start justify=Justify::Between gap=Space::Md surface=InlineSurface::Panel>
                                                                                                    <Inline gap=Space::Md shrink=true>
                                                                                                        <TelegramChannelAvatar channel_id=channel.id label=avatar_label />
                                                                                                            <Stack gap=Space::None shrink=true>
                                                                                                                    <Inline gap=Space::Sm wrap=true>
                                                                                                                        <InlinePanelTitle>{display_name}</InlinePanelTitle>
                                                                                                                        <Badge tone=Tone::Success style=BadgeStyle::Soft>"Active"</Badge>
                                                                                                                    </Inline>
                                                                                                                <FineText>{secondary}</FineText>
                                                                                                            </Stack>
                                                                                                    </Inline>
                                                                                                    <Inline gap=Space::Sm>
                                                                                                        <IconButton
                                                                                                            label="Remove"
                                                                                                            kind=ButtonKind::SoftDanger
                                                                                                            size=ButtonSize::Sm
                                                                                                            on_click=move |_| channel_pending_delete.set(Some(channel_for_remove.clone()))
                                                                                                        >
                                                                                                            <Icon icon=LuTrash2 width="1rem" height="1rem" />
                                                                                                        </IconButton>
                                                                                                    </Inline>
                                                                                                </Inline>
                                                                                            }
                                                                                        })
                                                                                            .collect_view()
                                                                                    }
                                                                                </Stack>
                                                                            </Show>
                                                                        </Stack>
                                                                        <Show when=move || telegram_link_value.get().as_ref().and_then(|result| result.as_ref().err()).is_some()>
                                                                            <Alert kind=AlertKind::Error>
                                                                                {move || {
                                                                                    telegram_link_value
                                                                                        .get()
                                                                                        .as_ref()
                                                                                        .and_then(|result| result.as_ref().err())
                                                                                        .map(|err| format!("Could not create Telegram link: {}", err))
                                                                                        .unwrap_or_default()
                                                                                }}
                                                                            </Alert>
                                                                        </Show>
                                                                        <Show when=move || delete_channel_value.get().as_ref().and_then(|result| result.as_ref().err()).is_some()>
                                                                            <Alert kind=AlertKind::Error>
                                                                                {move || {
                                                                                    delete_channel_value
                                                                                        .get()
                                                                                        .as_ref()
                                                                                        .and_then(|result| result.as_ref().err())
                                                                                        .map(|err| format!("Could not remove Telegram connection: {}", err))
                                                                                        .unwrap_or_default()
                                                                                }}
                                                                            </Alert>
                                                                        </Show>
                                                                    </Show>
                                                                </SettingsSubTabPanel>
                                                            </SettingsSubTabs>
                                                        </Card>
                                                    }
                                                >
                                                    <Stack gap=Space::Md>
                                                        <Card>
                                                            <Eyebrow>"Your account"</Eyebrow>
                                                            <Grid columns=GridCols::Two gap=Space::Xl top=Space::Lg>
                                                                <InfoItem icon=LuUser label="Name" value=profile_name.get_value() />
                                                                <InfoItem icon=LuMail label="Email" value=profile_email.get_value() />
                                                                <InfoItem icon=LuCalendar label="Member since" value=profile_member_since.get_value() />
                                                            </Grid>
                                                        </Card>

                                                        <Card>
                                                            <Eyebrow>"Company"</Eyebrow>
                                                            <BodyText>"View and update your company details."</BodyText>

                                                            <ActionForm action=update_action>
                                                                <Stack top=Space::Lg gap=Space::Md>
                                                                    <FormField label="Company name">
                                                                        <TextInput
                                                                            name="display_name"
                                                                            value=company_name
                                                                            placeholder="Company name"
                                                                            required=true
                                                                        />
                                                                    </FormField>
                                                                    <Show when=move || update_value.get().as_ref().and_then(|r| r.as_ref().err()).is_some()>
                                                                        <Alert kind=AlertKind::Error>
                                                                            {move || {
                                                                                update_value
                                                                                    .get()
                                                                                    .as_ref()
                                                                                    .and_then(|r| r.as_ref().err())
                                                                                    .map(|e| e.to_string())
                                                                                    .unwrap_or_default()
                                                                            }}
                                                                        </Alert>
                                                                    </Show>

                                                                    <FormField label="Organization number">
                                                                        <TextInput
                                                                            name="org_nr"
                                                                            value=org_number
                                                                            placeholder="e.g. 556123-4567"
                                                                        />
                                                                    </FormField>

                                                                    <FormField label="Jurisdiction">
                                                                        <TextInput
                                                                            name="jurisdiction"
                                                                            value=jurisdiction
                                                                            disabled=true
                                                                        />
                                                                        <FormHelp>"Jurisdiction is read-only in Phase 1."</FormHelp>
                                                                    </FormField>

                                                                    <Inline gap=Space::Md>
                                                                        <Button kind=ButtonKind::Primary button_type=ButtonType::Submit>
                                                                            "Save changes"
                                                                        </Button>
                                                                        <Show when=move || matches!(update_value.get(), Some(Ok(_)))>
                                                                            <SavedBadge>"Settings saved."</SavedBadge>
                                                                        </Show>
                                                                    </Inline>
                                                                </Stack>
                                                            </ActionForm>
                                                        </Card>
                                                    </Stack>
                                                </Show>
                                            </ScrollContent>
                                        </SidebarLayout>
                                    </DrawerContent>

                                    <DrawerSide drawer_id=SETTINGS_DRAWER_ID>
                                        <SettingsDrawerNav>
                                            <SettingsDrawerNavTop>
                                                <SettingsDrawerBrand href="/dashboard">"Finelor."</SettingsDrawerBrand>
                                                <SettingsNavSection>
                                                    <SettingsNavLink
                                                        href="/settings?tab=profile"
                                                        active=move || active_tab.get() == SettingsTab::ProfileCompany
                                                        drawer_id=SETTINGS_DRAWER_ID
                                                    >
                                                        <Icon icon=LuBuilding2 width="1rem" height="1rem" />
                                                        "Profile & company"
                                                    </SettingsNavLink>
                                                    <SettingsNavLink
                                                        href="/settings?tab=channels"
                                                        active=move || active_tab.get() == SettingsTab::Channels
                                                        drawer_id=SETTINGS_DRAWER_ID
                                                    >
                                                        <Icon icon=LuSend width="1rem" height="1rem" />
                                                        "Channels"
                                                    </SettingsNavLink>
                                                </SettingsNavSection>
                                            </SettingsDrawerNavTop>

                                            <SettingsDrawerNavBottom>
                                                <SettingsNavDivider />
                                                <SettingsNavSection>
                                                    <SettingsNavLink
                                                        href="/dashboard"
                                                        active=|| false
                                                    >
                                                        <Icon icon=LuSettings width="1rem" height="1rem" />
                                                        "Go to dashboard"
                                                    </SettingsNavLink>
                                                    <SettingsNavButton form="settings-logout-form-mobile">
                                                        <Icon icon=LuLogOut width="1rem" height="1rem" />
                                                        "Logout"
                                                    </SettingsNavButton>
                                                </SettingsNavSection>
                                                <ActionForm action=logout_action attr:id="settings-logout-form-mobile">
                                                    <input type="hidden" name="_logout" value="1" />
                                                </ActionForm>
                                            </SettingsDrawerNavBottom>
                                        </SettingsDrawerNav>
                                    </DrawerSide>
                                </Drawer>

                                <Show when=move || telegram_modal_link.get().is_some()>
                                    {move || {
                                        telegram_modal_link
                                            .get()
                                                    .map(|link| {
                                                let url_for_open = link.url.clone();
                                                let url_for_copy = link.url.clone();
                                                let copied = RwSignal::new(false);
                                                view! {
                                                    <Modal
                                                        close_label="Close Telegram connection dialog"
                                                        on_close=move |_| {
                                                            telegram_connect_error.set(None);
                                                            telegram_modal_link.set(None);
                                                        }
                                                    >
                                                        <ModalCloseButton
                                                            label="Close Telegram connection dialog"
                                                            on_click=move |_| {
                                                                telegram_connect_error.set(None);
                                                                telegram_modal_link.set(None);
                                                            }
                                                        >
                                                                <Icon icon=LuX width="1.25rem" height="1.25rem" />
                                                        </ModalCloseButton>

                                                        <ModalHeader>
                                                            <Inline gap=Space::Md shrink=true>
                                                                <Avatar>
                                                                    <Icon icon=LuSend width="1.25rem" height="1.25rem" />
                                                                </Avatar>
                                                                    <div>
                                                                        <PanelTitle>"Establish new connection"</PanelTitle>
                                                                        <FineText>"Telegram"</FineText>
                                                                    </div>
                                                            </Inline>
                                                        </ModalHeader>

                                                        <ModalInstruction>
                                                            <InlineText>
                                                                <Icon icon=LuScanQrCode width="1rem" height="1rem" />
                                                                "Scan with your phone"
                                                            </InlineText>
                                                        </ModalInstruction>

                                                        <QrCodeBox svg=link.qr_svg.clone() />

                                                        <Grid columns=GridCols::EqualThreeCompact gap=Space::Sm top=Space::Md>
                                                            <IconText icon=LuQrCode>"Scan"</IconText>
                                                            <IconText icon=LuBot>"Open bot"</IconText>
                                                            <IconText icon=LuSend>"Start sending"</IconText>
                                                        </Grid>

                                                        <Show
                                                            when=move || telegram_connect_error.get().is_some()
                                                            fallback=move || view! { <ModalActionGap /> }
                                                        >
                                                            <Alert kind=AlertKind::Error>
                                                                {move || telegram_connect_error.get().unwrap_or_default()}
                                                            </Alert>
                                                        </Show>

                                                        <ModalActions>
                                                                <LinkButton
                                                                    href=url_for_open
                                                                    target="_blank"
                                                                    rel="noopener noreferrer"
                                                                    kind=ButtonKind::Primary
                                                                >
                                                                    "Open here"
                                                                    <Icon icon=LuArrowUpRight width="1rem" height="1rem" />
                                                                </LinkButton>
                                                                <ActionButton on_click=move |_| {
                                                                    copy_to_clipboard(&url_for_copy);
                                                                    copied.set(true);
                                                                    run_after_ms(1_500, move || copied.set(false));
                                                                }>
                                                                    <Show
                                                                        when=move || copied.get()
                                                                        fallback=move || view! {
                                                                            <Icon icon=LuCopy width="1.1rem" height="1.1rem" />
                                                                            "Copy link"
                                                                        }
                                                                    >
                                                                        <Icon icon=LuCheck width="1.1rem" height="1.1rem" />
                                                                        "Copied"
                                                                    </Show>
                                                                </ActionButton>
                                                        </ModalActions>

                                                        <CenterFineText>"No Telegram on this device? Scan the QR from your phone."</CenterFineText>
                                                    </Modal>
                                                }.into_any()
                                            })
                                            .unwrap_or_else(|| ().into_any())
                                    }}
                                </Show>

                                <Show when=move || channel_notice.get().is_some()>
                                    <ToastViewport>
                                        <ToastAlert kind=AlertKind::Success>
                                            {move || channel_notice.get().unwrap_or_default()}
                                        </ToastAlert>
                                    </ToastViewport>
                                </Show>

                                <Show when=move || channel_pending_delete.get().is_some()>
                                    {move || {
                                        channel_pending_delete
                                            .get()
                                            .map(|channel| {
                                                let subtitle = telegram_channel_identity_label(&channel);
                                                view! {
                                                    <Modal
                                                        close_label="Cancel removing Telegram connection"
                                                        on_close=move |_| channel_pending_delete.set(None)
                                                    >
                                                        <ModalCloseButton
                                                            label="Cancel removing Telegram connection"
                                                            on_click=move |_| channel_pending_delete.set(None)
                                                        >
                                                            <Icon icon=LuX width="1.25rem" height="1.25rem" />
                                                        </ModalCloseButton>

                                                        <ModalHeader>
                                                            <Inline gap=Space::Md shrink=true>
                                                                <Avatar>
                                                                    <Icon icon=LuTrash2 width="1.1rem" height="1.1rem" />
                                                                </Avatar>
                                                                <div>
                                                                    <PanelTitle>"Remove Telegram connection"</PanelTitle>
                                                                    <FineText>{subtitle}</FineText>
                                                                </div>
                                                            </Inline>
                                                        </ModalHeader>

                                                        <ModalTitle>"Remove this connection?"</ModalTitle>
                                                        <BodyText>"Finelor will stop recognizing this Telegram chat for future messages. Existing documents stay unchanged."</BodyText>

                                                        <ActionForm action=delete_channel_action>
                                                            <input type="hidden" name="channel_id" value=channel.id.to_string() />
                                                            <ModalActions>
                                                                <ActionButton on_click=move |_| channel_pending_delete.set(None)>
                                                                    "Cancel"
                                                                </ActionButton>
                                                                <Button kind=ButtonKind::SoftDanger button_type=ButtonType::Submit>
                                                                    <Icon icon=LuTrash2 width="1rem" height="1rem" />
                                                                    "Remove"
                                                                </Button>
                                                            </ModalActions>
                                                        </ActionForm>
                                                    </Modal>
                                                }.into_any()
                                            })
                                            .unwrap_or_else(|| ().into_any())
                                    }}
                                </Show>
                            </>
                        }
                        .into_any()
                    }
                    Some(Err(err)) => view! {
                        <Alert kind=AlertKind::Error>
                            {format!("Failed to load settings: {}", err)}
                        </Alert>
                    }
                    .into_any(),
                    None => view! { <HiddenPlaceholder /> }.into_any(),
                }}
            </Suspense>
        </div>
    }
}
