use leptos::prelude::*;
use leptos_icons::Icon;

#[derive(Clone, Copy)]
pub enum CardPadding {
    Compact,
    Comfortable,
}

impl CardPadding {
    fn body_class(self) -> &'static str {
        match self {
            Self::Compact => "card-body gap-3 p-4",
            Self::Comfortable => "card-body",
        }
    }
}

#[component]
pub fn Card(
    children: Children,
    #[prop(default = CardPadding::Comfortable)] padding: CardPadding,
) -> impl IntoView {
    view! {
        <section class="card card-border bg-base-100 border-base-300 shadow-sm">
            <div class=padding.body_class()>
                {children()}
            </div>
        </section>
    }
}

#[component]
pub fn ActionCard(
    children: Children,
    href: String,
    #[prop(default = CardPadding::Comfortable)] padding: CardPadding,
) -> impl IntoView {
    view! {
        <a href=href class="card card-border bg-base-100 border-base-300 shadow-sm transition hover:border-primary/40">
            <div class=padding.body_class()>
                {children()}
            </div>
        </a>
    }
}

#[component]
pub fn NestedCard(children: Children) -> impl IntoView {
    view! {
        <div class="card bg-base-100 shadow-sm">
            <div class="card-body">
                {children()}
            </div>
        </div>
    }
}

#[component]
pub fn Divider() -> impl IntoView {
    view! { <div class="divider my-2"></div> }
}

#[component]
pub fn Modal(
    children: Children,
    close_label: &'static str,
    on_close: impl Fn(leptos::ev::MouseEvent) + Clone + 'static,
) -> impl IntoView {
    let close_backdrop = on_close.clone();

    view! {
        <div class="modal modal-open" role="dialog" aria-modal="true">
            <div class="modal-box w-11/12 max-w-lg">
                {children()}
            </div>
            <button
                type="button"
                class="modal-backdrop"
                aria-label=close_label
                on:click=close_backdrop
            >
                "close"
            </button>
        </div>
    }
}

#[component]
pub fn ModalCloseButton(
    children: Children,
    label: &'static str,
    on_click: impl Fn(leptos::ev::MouseEvent) + 'static,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class="btn btn-sm btn-circle btn-ghost absolute right-2 top-2"
            aria-label=label
            on:click=on_click
        >
            {children()}
        </button>
    }
}

#[component]
pub fn ModalActions(children: Children) -> impl IntoView {
    view! {
        <div class="modal-action mt-3 grid grid-cols-2 gap-2">
            {children()}
        </div>
    }
}

#[component]
pub fn ModalActionGap() -> impl IntoView {
    view! { <div class="mt-3"></div> }
}

#[component]
pub fn ModalHeader(children: Children) -> impl IntoView {
    view! {
        <div class="flex items-center justify-between gap-3 pr-8">
            {children()}
        </div>
    }
}

#[component]
pub fn Avatar(
    #[prop(optional)] src: Option<String>,
    #[prop(optional)] alt: Option<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    match src {
        Some(src) => view! {
            <div class="avatar">
                <div class="size-9 shrink-0 overflow-hidden rounded-full">
                    <img src=src alt=alt.unwrap_or_default() class="size-full object-cover" />
                </div>
            </div>
        }
        .into_any(),
        None => view! {
                <div class="avatar placeholder">
                    <div class="grid size-9 shrink-0 place-items-center rounded-full bg-info text-info-content leading-none [&_svg]:block">
                    {children.map(|children| children()).unwrap_or_else(|| ().into_any())}
                </div>
            </div>
        }
        .into_any(),
    }
}

#[component]
pub fn QrCodeBox(svg: String) -> impl IntoView {
    view! {
        <div class="mx-auto mt-5 flex size-60 items-center justify-center p-1 sm:size-72 [&_svg]:h-full [&_svg]:w-full">
            <div inner_html=svg></div>
        </div>
    }
}

#[component]
pub fn IconText(icon: icondata::Icon, children: Children) -> impl IntoView {
    view! {
        <div class="flex flex-col items-center gap-1 text-[10px] font-semibold text-base-content/65">
            <Icon icon=icon width="1rem" height="1rem" />
            <span>{children()}</span>
        </div>
    }
}
