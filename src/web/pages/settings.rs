use crate::web::client::{copy_to_clipboard, redirect, run_after_ms, subscribe_app_events};
use crate::web::components::ui::{
    ActionButton, Alert, AlertKind, Align, Avatar, Badge, BadgeStyle, Banner, BannerProminence,
    BodyText, Button, ButtonKind, ButtonSize, ButtonType, Card, CenterFineText, Cluster,
    DesktopSidebar, Drawer, DrawerContent, DrawerSide, Eyebrow, FineText, FormField, FormHelp,
    Grid, GridCols, HiddenPlaceholder, IconButton, IconText, InfoItem, Inline, InlineSurface,
    InlineText, Justify, LinkButton, LoadingCard, Modal, ModalActionGap, ModalActions,
    ModalCloseButton, ModalHeader, ModalInstruction, ModalTitle, NestedCard, PanelTitle, QrCodeBox,
    SETTINGS_DRAWER_ID, SavedBadge, ScrollContent, SettingsDrawerBrand, SettingsDrawerNav,
    SettingsDrawerNavBottom, SettingsDrawerNavTop, SettingsNav, SettingsNavButton,
    SettingsNavDivider, SettingsNavLink, SettingsNavSection, SettingsSubTabButton,
    SettingsSubTabList, SettingsSubTabPanel, SettingsSubTabs, SidebarLayout, Space, Stack,
    TextInput, ToastAlert, ToastViewport, Tone,
};
use crate::web::server::auth::{AuthUser, Logout, get_session_user};
use crate::web::server::channels::{
    CompanyChannel, CreateTelegramConnectLink, DeleteCompanyChannel, MessagingProviderStatus,
    get_messaging_provider_status, get_telegram_channel_avatar, list_company_channels,
};
use crate::web::server::settings::{CompanySettings, UpdateCompanySettings, get_company_settings};
use icondata::{
    LuArrowUpRight, LuBot, LuBuilding2, LuCalendar, LuCheck, LuCircleAlert, LuCopy, LuFileText,
    LuLogOut, LuMail, LuMessageCircle, LuPlus, LuQrCode, LuScanQrCode, LuSend, LuSettings, LuSlack,
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
    messaging_status: MessagingProviderStatus,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    ProfileCompany,
    Channels,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChannelSettingsTab {
    Telegram,
    Slack,
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

fn slack_channel_display_name(channel: &CompanyChannel) -> String {
    channel
        .slack_channel_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            if value.starts_with('#') {
                value.to_string()
            } else {
                format!("#{value}")
            }
        })
        .unwrap_or_else(|| "Slack channel".to_string())
}

fn slack_channel_is_listable(channel: &CompanyChannel) -> bool {
    if channel.channel_type != "SLACK" || !channel.active {
        return false;
    }
    let channel_kind = channel
        .slack_channel_type
        .as_deref()
        .map(|value| value.to_ascii_lowercase());
    if matches!(channel_kind.as_deref(), Some("im") | Some("mpim")) {
        return false;
    }
    !channel.channel_identifier.starts_with('D')
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
                messaging_status: get_messaging_provider_status().await?,
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
                        let active_slack_channels = settings
                            .channels
                            .iter()
                            .filter(|channel| slack_channel_is_listable(channel))
                            .cloned()
                            .collect::<Vec<_>>();
                        let has_active_telegram_channels = !active_telegram_channels.is_empty();
                        let telegram_channel_count = active_telegram_channels.len();
                        let has_active_slack_channels = !active_slack_channels.is_empty();
                        let telegram_is_active_provider =
                            settings.messaging_status.active_provider == "telegram";
                        let slack_is_active_provider =
                            settings.messaging_status.active_provider == "slack";
                        let slack_workspace_title = StoredValue::new(
                            if has_active_slack_channels {
                                settings
                                    .messaging_status
                                    .slack_workspace_name
                                    .clone()
                                    .unwrap_or_else(|| "Slack workspace".to_string())
                            } else {
                                "Slack workspace".to_string()
                            },
                        );
                        let slack_workspace_url = StoredValue::new(
                            settings
                                .messaging_status
                                .slack_workspace_url
                                .clone()
                                .unwrap_or_else(|| "Unavailable".to_string()),
                        );
                        let active_telegram_channels = RwSignal::new(active_telegram_channels);
                        let active_slack_channels = RwSignal::new(active_slack_channels);
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
                                                                        active=move || active_channel_tab.get() == ChannelSettingsTab::Slack
                                                                        on_click=move |_| active_channel_tab.set(ChannelSettingsTab::Slack)
                                                                    >
                                                                        <Icon icon=LuSlack width="1rem" height="1rem" />
                                                                        "Slack"
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
                                                                                when=move || active_channel_tab.get() == ChannelSettingsTab::Slack
                                                                                fallback=move || view! {
                                                                                    <BodyText>"API/Webhook Settings - Coming Soon"</BodyText>
                                                                                }
                                                                            >
                                                                                <Stack top=Space::Md gap=Space::Md>
                                                                                    <Inline justify=Justify::Between stack_mobile=true>
                                                                                        <Stack gap=Space::None>
                                                                                            <Cluster gap=Space::Sm>
                                                                                                <PanelTitle>{slack_workspace_title.get_value()}</PanelTitle>
                                                                                                <Badge
                                                                                                    tone=if slack_is_active_provider { Tone::Success } else { Tone::Neutral }
                                                                                                    style=BadgeStyle::Soft
                                                                                                >
                                                                                                    {if slack_is_active_provider { "Active" } else { "Inactive" }}
                                                                                                </Badge>
                                                                                            </Cluster>
                                                                                            <FineText>
                                                                                                {format!("Workspace URL: {}", slack_workspace_url.get_value())}
                                                                                            </FineText>
                                                                                        </Stack>
                                                                                    </Inline>

                                                                                    <Show
                                                                                        when=move || has_active_slack_channels
                                                                                        fallback=move || view! {
                                                                                            <Inline surface=InlineSurface::Panel>
                                                                                                <Stack gap=Space::None shrink=true>
                                                                                                    <PanelTitle>"No Slack channels yet"</PanelTitle>
                                                                                                    <FineText>"Slack channels appear here after messages are received from known connected channels."</FineText>
                                                                                                </Stack>
                                                                                            </Inline>
                                                                                        }
                                                                                    >
                                                                                        <Stack gap=Space::Sm>
                                                                                            {move || active_slack_channels
                                                                                                .get()
                                                                                                .into_iter()
                                                                                                .map(|channel| {
                                                                                                    let channel_for_remove = channel.clone();
                                                                                                    let display_name = slack_channel_display_name(&channel);
                                                                                                    let subtitle = "Slack channel".to_string();
                                                                                                    view! {
                                                                                                        <Inline align=Align::Start justify=Justify::Between gap=Space::Md surface=InlineSurface::Panel>
                                                                                                            <Inline gap=Space::Md shrink=true>
                                                                                                                <Avatar>
                                                                                                                    <Icon icon=LuMessageCircle width="1.1rem" height="1.1rem" />
                                                                                                                </Avatar>
                                                                                                                <Stack gap=Space::None shrink=true>
                                                                                                                    <Cluster gap=Space::Sm>
                                                                                                                        <PanelTitle>{display_name}</PanelTitle>
                                                                                                                        <Badge tone=Tone::Success style=BadgeStyle::Soft>"Active"</Badge>
                                                                                                                    </Cluster>
                                                                                                                    <FineText>{subtitle}</FineText>
                                                                                                                </Stack>
                                                                                                            </Inline>
                                                                                                            <Inline gap=Space::Sm>
                                                                                                                <IconButton
                                                                                                                    label="Remove"
                                                                                                                    kind=ButtonKind::SoftDanger
                                                                                                                    size=ButtonSize::Sm
                                                                                                                    disabled=!slack_is_active_provider
                                                                                                                    on_click=move |_| {
                                                                                                                        if slack_is_active_provider {
                                                                                                                            channel_pending_delete.set(Some(channel_for_remove.clone()))
                                                                                                                        }
                                                                                                                    }
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

                                                                                    <Show when=move || !slack_is_active_provider>
                                                                                        <Banner
                                                                                            title="Slack provider is inactive in server config".to_string()
                                                                                            subtitle="Controls are disabled until Slack is set as the active messaging provider.".to_string()
                                                                                            icon=LuCircleAlert
                                                                                            tone=Tone::Warning
                                                                                            prominence=BannerProminence::Spacious
                                                                                        />
                                                                                    </Show>
                                                                                </Stack>
                                                                            </Show>
                                                                        }
                                                                    >
                                                                        <Stack top=Space::Md gap=Space::Md>
                                                                            <Inline justify=Justify::Between stack_mobile=true>
                                                                                <Stack gap=Space::None>
                                                                                    <Cluster gap=Space::Sm>
                                                                                        <PanelTitle>"Telegram connections"</PanelTitle>
                                                                                        <Badge
                                                                                            tone=if telegram_is_active_provider { Tone::Success } else { Tone::Neutral }
                                                                                            style=BadgeStyle::Soft
                                                                                        >
                                                                                            {if telegram_is_active_provider { "Active" } else { "Inactive" }}
                                                                                        </Badge>
                                                                                    </Cluster>
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
                                                                                    disabled=!telegram_is_active_provider
                                                                                    on_click=move |_| {
                                                                                        if telegram_is_active_provider {
                                                                                            telegram_link_action.dispatch(CreateTelegramConnectLink {});
                                                                                        }
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
                                                                                    <NestedCard>
                                                                                        <Inline align=Align::Start gap=Space::Md>
                                                                                            <Avatar>
                                                                                                <Icon icon=LuSend width="1.1rem" height="1.1rem" />
                                                                                            </Avatar>
                                                                                            <Stack gap=Space::None shrink=true>
                                                                                                <PanelTitle>"Connect Telegram"</PanelTitle>
                                                                                                <FineText>"Add a Telegram chat so Finelor can receive documents and send updates."</FineText>
                                                                                            </Stack>
                                                                                        </Inline>
                                                                                    </NestedCard>
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
                                                                                                            <Cluster gap=Space::Sm>
                                                                                                                <PanelTitle>{display_name}</PanelTitle>
                                                                                                                <Badge tone=Tone::Success style=BadgeStyle::Soft>"Active"</Badge>
                                                                                                            </Cluster>
                                                                                                            <FineText>{secondary}</FineText>
                                                                                                        </Stack>
                                                                                                    </Inline>
                                                                                                    <Inline gap=Space::Sm>
                                                                                                        <IconButton
                                                                                                            label="Remove"
                                                                                                            kind=ButtonKind::SoftDanger
                                                                                                            size=ButtonSize::Sm
                                                                                                            disabled=!telegram_is_active_provider
                                                                                                            on_click=move |_| {
                                                                                                                if telegram_is_active_provider {
                                                                                                                    channel_pending_delete.set(Some(channel_for_remove.clone()))
                                                                                                                }
                                                                                                            }
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
                                                                            <Show when=move || !telegram_is_active_provider>
                                                                                <Banner
                                                                                    title="Telegram provider is inactive in server config".to_string()
                                                                                    subtitle="Controls are disabled until Telegram is set as the active messaging provider.".to_string()
                                                                                    icon=LuCircleAlert
                                                                                    tone=Tone::Warning
                                                                                    prominence=BannerProminence::Spacious
                                                                                />
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
                                                let is_telegram = channel.channel_type == "TELEGRAM";
                                                let subtitle = if is_telegram {
                                                    telegram_channel_identity_label(&channel)
                                                } else {
                                                    channel
                                                        .display_name
                                                        .clone()
                                                        .unwrap_or_else(|| format!("Slack · {}", channel.channel_identifier))
                                                };
                                                let remove_title = if is_telegram {
                                                    "Remove Telegram connection"
                                                } else {
                                                    "Remove Slack connection"
                                                };
                                                view! {
                                                    <Modal
                                                        close_label="Cancel removing connection"
                                                        on_close=move |_| channel_pending_delete.set(None)
                                                    >
                                                        <ModalCloseButton
                                                            label="Cancel removing connection"
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
                                                                    <PanelTitle>{remove_title}</PanelTitle>
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
