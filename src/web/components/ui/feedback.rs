use crate::web::components::ui::Card;
use icondata::Icon as IconData;
use leptos::prelude::*;
use leptos_icons::Icon;

#[derive(Clone, Copy)]
pub enum AlertKind {
    Info,
    Success,
    Warning,
    Error,
}

impl AlertKind {
    fn class(self) -> &'static str {
        match self {
            Self::Info => "alert-info",
            Self::Success => "alert-success",
            Self::Warning => "alert-warning",
            Self::Error => "alert-error",
        }
    }
}

#[component]
pub fn Alert(
    children: Children,
    #[prop(default = AlertKind::Info)] kind: AlertKind,
) -> impl IntoView {
    view! {
        <div class=format!("alert {}", kind.class())>
            <div>{children()}</div>
        </div>
    }
}

#[component]
pub fn ToastViewport(children: Children) -> impl IntoView {
    view! { <div class="toast toast-bottom toast-end z-50">{children()}</div> }
}

#[component]
pub fn ToastAlert(
    children: Children,
    #[prop(default = AlertKind::Info)] kind: AlertKind,
) -> impl IntoView {
    view! {
        <div class=format!("alert shadow-lg {}", kind.class())>
            <div>{children()}</div>
        </div>
    }
}

#[derive(Clone, Copy)]
pub enum Tone {
    Neutral,
    Info,
    Success,
    Warning,
    Error,
}

impl Tone {
    fn badge_tone_class(self) -> &'static str {
        match self {
            Self::Neutral => "",
            Self::Info => "badge-info",
            Self::Success => "badge-success",
            Self::Warning => "badge-warning",
            Self::Error => "badge-error",
        }
    }

    fn alert_tone_class(self) -> &'static str {
        match self {
            Self::Neutral => "",
            Self::Info => "alert-info",
            Self::Success => "alert-success",
            Self::Warning => "alert-warning",
            Self::Error => "alert-error",
        }
    }

    fn banner_border_class(self) -> &'static str {
        match self {
            Self::Neutral => "border-base-content/20",
            Self::Info => "border-info/40",
            Self::Success => "border-success/40",
            Self::Warning => "border-warning/45",
            Self::Error => "border-error/40",
        }
    }

    fn banner_title_class(self) -> &'static str {
        "font-bold text-base-content/85"
    }

    fn banner_subtitle_class(self) -> &'static str {
        "mt-1 text-sm text-base-content/60"
    }

    fn banner_icon_class(self) -> &'static str {
        match self {
            Self::Neutral => {
                "flex size-10 shrink-0 items-center justify-center rounded-box border border-base-content/25 bg-base-content/10 text-base-content"
            }
            Self::Info => {
                "flex size-10 shrink-0 items-center justify-center rounded-box border border-info/35 bg-info/15 text-info"
            }
            Self::Success => {
                "flex size-10 shrink-0 items-center justify-center rounded-box border border-success/35 bg-success/15 text-success"
            }
            Self::Warning => {
                "flex size-10 shrink-0 items-center justify-center rounded-box border border-warning/40 bg-warning/15 text-warning"
            }
            Self::Error => {
                "flex size-10 shrink-0 items-center justify-center rounded-box border border-error/35 bg-error/15 text-error"
            }
        }
    }

    fn banner_trailing_icon_class(self) -> &'static str {
        match self {
            Self::Neutral => {
                "ml-auto hidden size-9 shrink-0 items-center justify-center rounded-full border border-base-content/25 bg-base-content/10 text-base-content sm:flex"
            }
            Self::Info => {
                "ml-auto hidden size-9 shrink-0 items-center justify-center rounded-full border border-info/35 bg-info/15 text-info sm:flex"
            }
            Self::Success => {
                "ml-auto hidden size-9 shrink-0 items-center justify-center rounded-full border border-success/35 bg-success/15 text-success sm:flex"
            }
            Self::Warning => {
                "ml-auto hidden size-9 shrink-0 items-center justify-center rounded-full border border-warning/40 bg-warning/15 text-warning sm:flex"
            }
            Self::Error => {
                "ml-auto hidden size-9 shrink-0 items-center justify-center rounded-full border border-error/35 bg-error/15 text-error sm:flex"
            }
        }
    }
}

#[derive(Clone, Copy)]
pub enum BadgeStyle {
    Solid,
    Soft,
    Outline,
}

impl BadgeStyle {
    fn class(self) -> &'static str {
        match self {
            Self::Solid => "",
            Self::Soft => "badge-soft",
            Self::Outline => "badge-outline",
        }
    }
}

#[component]
pub fn Badge(
    children: Children,
    #[prop(default = Tone::Neutral)] tone: Tone,
    #[prop(default = BadgeStyle::Solid)] style: BadgeStyle,
) -> impl IntoView {
    let class = format!("badge {} {}", tone.badge_tone_class(), style.class());

    view! {
        <span class=class>{children()}</span>
    }
}

#[component]
pub fn StatusBadge(status: String) -> impl IntoView {
    let tone = match status.as_str() {
        "EXPORT_READY" | "EXPORTED" => Tone::Success,
        "PENDING_HUMAN_REVIEW" => Tone::Warning,
        "FAILED" => Tone::Error,
        _ => Tone::Info,
    };

    view! { <Badge tone=tone>{status}</Badge> }
}

#[component]
pub fn SavedBadge(children: Children) -> impl IntoView {
    view! { <Badge tone=Tone::Success>{children()}</Badge> }
}

#[derive(Clone, Copy)]
pub enum BannerProminence {
    Compact,
    Spacious,
}

impl BannerProminence {
    fn class(self) -> &'static str {
        match self {
            Self::Compact => "alert alert-soft items-start gap-4 border p-5",
            Self::Spacious => "alert alert-soft items-center gap-4 border p-6",
        }
    }
}

#[component]
pub fn Banner(
    title: String,
    #[prop(optional)] subtitle: Option<String>,
    icon: IconData,
    #[prop(default = Tone::Info)] tone: Tone,
    #[prop(default = BannerProminence::Compact)] prominence: BannerProminence,
    #[prop(optional)] trailing_icon: Option<IconData>,
) -> impl IntoView {
    let class = format!(
        "{} {} {}",
        prominence.class(),
        tone.alert_tone_class(),
        tone.banner_border_class()
    );
    let title_class = tone.banner_title_class();
    let subtitle_class = tone.banner_subtitle_class();
    let icon_class = tone.banner_icon_class();
    let trailing_class = tone.banner_trailing_icon_class();
    let has_subtitle = subtitle.is_some();
    let subtitle_text = subtitle.unwrap_or_default();
    let has_trailing_icon = trailing_icon.is_some();
    let trailing = trailing_icon.unwrap_or(icon);

    view! {
        <div role="status" class=class>
            <div class=icon_class>
                <Icon icon=icon width="1.25rem" height="1.25rem" />
            </div>
            <div class="min-w-0">
                <p class=title_class>{title}</p>
                <Show when=move || has_subtitle>
                    <p class=subtitle_class>{subtitle_text.clone()}</p>
                </Show>
            </div>
            <Show when=move || has_trailing_icon>
                <div class=trailing_class>
                    <Icon icon=trailing width="1.1rem" height="1.1rem" />
                </div>
            </Show>
        </div>
    }
}

#[component]
pub fn LoadingCard(message: &'static str) -> impl IntoView {
    view! {
        <Card>
            <div class="flex items-center gap-3 text-sm text-base-content/70">
                <span class="loading loading-spinner"></span>
                <span>{message}</span>
            </div>
        </Card>
    }
}

#[component]
pub fn EmptyState(children: Children) -> impl IntoView {
    view! {
        <Card>
            <p class="text-base text-base-content/65">{children()}</p>
        </Card>
    }
}

#[component]
pub fn HiddenPlaceholder() -> impl IntoView {
    view! { <div class="hidden" aria-hidden="true"></div> }
}
