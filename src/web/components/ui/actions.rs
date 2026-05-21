use leptos::prelude::*;

#[derive(Clone, Copy)]
pub enum ButtonKind {
    Default,
    Primary,
    Ghost,
    Soft,
    Danger,
    SoftDanger,
}

impl ButtonKind {
    fn class(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Primary => "btn-primary",
            Self::Ghost => "btn-ghost",
            Self::Soft => "btn-soft",
            Self::Danger => "btn-error",
            Self::SoftDanger => "btn-soft btn-error",
        }
    }
}

#[derive(Clone, Copy)]
pub enum ButtonSize {
    Sm,
    Md,
    Lg,
}

impl ButtonSize {
    fn class(self) -> &'static str {
        match self {
            Self::Sm => "btn-sm",
            Self::Md => "",
            Self::Lg => "btn-lg",
        }
    }
}

#[derive(Clone, Copy)]
pub enum ButtonType {
    Button,
    Submit,
    Reset,
}

impl ButtonType {
    fn attr(self) -> &'static str {
        match self {
            Self::Button => "button",
            Self::Submit => "submit",
            Self::Reset => "reset",
        }
    }
}

fn button_class(kind: ButtonKind, size: ButtonSize, full_width: bool, square: bool) -> String {
    let mut classes = vec!["btn"];
    if !kind.class().is_empty() {
        classes.push(kind.class());
    }
    if !size.class().is_empty() {
        classes.push(size.class());
    }
    if full_width {
        classes.push("w-full");
    }
    if square {
        classes.push("btn-square");
    }
    classes.join(" ")
}

#[component]
pub fn Button(
    children: Children,
    #[prop(default = ButtonKind::Default)] kind: ButtonKind,
    #[prop(default = ButtonSize::Md)] size: ButtonSize,
    #[prop(default = ButtonType::Button)] button_type: ButtonType,
    #[prop(default = false)] full_width: bool,
    #[prop(default = false)] disabled: bool,
    #[prop(default = false)] loading: bool,
    #[prop(optional)] form: Option<&'static str>,
) -> impl IntoView {
    let class = button_class(kind, size, full_width, false);

    view! {
        <button type=button_type.attr() form=form class=class disabled=disabled>
            <Show when=move || loading>
                <span class="loading loading-spinner loading-sm"></span>
            </Show>
            {children()}
        </button>
    }
}

#[component]
pub fn ActionButton(
    children: Children,
    on_click: impl Fn(leptos::ev::MouseEvent) + 'static,
    #[prop(default = ButtonKind::Default)] kind: ButtonKind,
    #[prop(default = ButtonSize::Md)] size: ButtonSize,
    #[prop(default = false)] full_width: bool,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class=button_class(kind, size, full_width, false)
            on:click=on_click
        >
            {children()}
        </button>
    }
}

#[component]
pub fn IconButton(
    children: Children,
    label: &'static str,
    on_click: impl Fn(leptos::ev::MouseEvent) + 'static,
    #[prop(default = ButtonKind::Ghost)] kind: ButtonKind,
    #[prop(default = ButtonSize::Md)] size: ButtonSize,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class=button_class(kind, size, false, true)
            aria-label=label
            on:click=on_click
        >
            {children()}
        </button>
    }
}

#[component]
pub fn LinkButton(
    children: Children,
    href: String,
    #[prop(default = ButtonKind::Default)] kind: ButtonKind,
    #[prop(default = ButtonSize::Md)] size: ButtonSize,
    #[prop(default = false)] full_width: bool,
    #[prop(optional)] target: Option<&'static str>,
    #[prop(optional)] rel: Option<&'static str>,
) -> impl IntoView {
    view! {
        <a href=href target=target rel=rel class=button_class(kind, size, full_width, false)>
            {children()}
        </a>
    }
}

#[component]
pub fn TextLink(children: Children, href: &'static str) -> impl IntoView {
    view! {
        <a href=href class="link link-hover text-xs font-semibold text-base-content/65">
            {children()}
        </a>
    }
}
