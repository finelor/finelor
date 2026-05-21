use leptos::prelude::*;
use leptos_router::components::A;

pub const SETTINGS_DRAWER_ID: &str = "settings-drawer";

#[component]
pub fn AppBar(children: Children) -> impl IntoView {
    let scrolled = RwSignal::new(false);

    #[cfg(target_arch = "wasm32")]
    {
        scrolled.set(leptos_dom::helpers::window().scroll_y().unwrap_or_default() > 0.0);

        let handle = leptos_dom::helpers::window_event_listener(leptos::ev::scroll, move |_| {
            scrolled.set(leptos_dom::helpers::window().scroll_y().unwrap_or_default() > 0.0);
        });
        on_cleanup(move || handle.remove());
    }

    view! {
        <header class=move || {
            if scrolled.get() {
                "sticky top-0 z-40 border-b-2 border-base-300 bg-base-100 shadow-sm"
            } else {
                "sticky top-0 z-40 border-b-2 border-base-300 bg-base-100"
            }
        }>
            <div class="navbar mx-auto w-full max-w-6xl px-4 py-3">
                {children()}
            </div>
        </header>
    }
}

#[component]
pub fn AppBarStart(children: Children) -> impl IntoView {
    view! { <div class="navbar-start">{children()}</div> }
}

#[component]
pub fn AppBarStartCluster(children: Children) -> impl IntoView {
    view! { <div class="flex items-center gap-2">{children()}</div> }
}

#[component]
pub fn AppBarDrawerButton(
    children: Children,
    drawer_id: &'static str,
    label: &'static str,
) -> impl IntoView {
    view! {
        <label for=drawer_id aria-label=label class="btn btn-square btn-ghost lg:hidden">
            {children()}
        </label>
    }
}

#[component]
pub fn AppBarEnd(children: Children) -> impl IntoView {
    view! { <div class="navbar-end">{children()}</div> }
}

#[component]
pub fn BrandLink(children: Children, href: &'static str) -> impl IntoView {
    view! {
        <a href=href class="text-2xl font-extrabold tracking-tight text-base-content md:text-3xl">
            {children()}
        </a>
    }
}

#[component]
pub fn AvatarLink(children: Children, href: &'static str, label: &'static str) -> impl IntoView {
    view! {
        <a href=href class="avatar placeholder" aria-label=label>
            <div class="flex w-10 items-center justify-center rounded-full bg-primary text-sm font-semibold text-primary-content">
                {children()}
            </div>
        </a>
    }
}

#[component]
pub fn Footer(children: Children, wide: bool) -> impl IntoView {
    let max_width = if wide { "max-w-6xl" } else { "max-w-5xl" };
    view! {
        <footer class="border-t-2 border-base-300 bg-base-100 px-4 py-6 text-xs text-base-content/55">
            <div class=format!("mx-auto flex w-full {} flex-col gap-2 md:flex-row md:items-center md:justify-between", max_width)>
                {children()}
            </div>
        </footer>
    }
}

#[component]
pub fn FooterLinks(children: Children) -> impl IntoView {
    view! { <div class="flex flex-wrap gap-3">{children()}</div> }
}

#[component]
pub fn FooterLink(children: Children, href: &'static str) -> impl IntoView {
    view! { <a href=href class="link link-hover">{children()}</a> }
}

#[component]
pub fn Tabs(children: Children) -> impl IntoView {
    view! { <div role="tablist" class="tabs">{children()}</div> }
}

#[component]
pub fn TabLink(children: Children, href: &'static str, active: bool) -> impl IntoView {
    let class = if active { "tab tab-active" } else { "tab" };
    view! { <a role="tab" class=class href=href>{children()}</a> }
}

#[component]
pub fn RadioTabs(children: Children) -> impl IntoView {
    view! { <div class="mt-4 tabs">{children()}</div> }
}

#[component]
pub fn RadioTab(
    children: Children,
    name: &'static str,
    #[prop(default = false)] checked: bool,
) -> impl IntoView {
    view! {
        <label class="tab">
            <input type="radio" name=name checked=checked />
            {children()}
        </label>
    }
}

#[component]
pub fn TabContent(children: Children) -> impl IntoView {
    view! { <div class="tab-content">{children()}</div> }
}

#[component]
pub fn ListMenu(children: Children) -> impl IntoView {
    view! { <ul class="menu">{children()}</ul> }
}

#[component]
pub fn SettingsNav(children: Children) -> impl IntoView {
    view! {
        <nav class="card card-border bg-base-100 border-base-300 shadow-sm">
            <div class="card-body gap-2 p-3">
                {children()}
            </div>
        </nav>
    }
}

#[component]
pub fn SettingsNavSection(children: Children) -> impl IntoView {
    view! { <ul class="menu w-full gap-1 p-0">{children()}</ul> }
}

#[component]
pub fn SettingsNavDivider() -> impl IntoView {
    view! { <div class="divider mb-2 mt-4"></div> }
}

#[component]
pub fn SettingsDrawerNav(children: Children) -> impl IntoView {
    view! {
        <nav class="flex min-h-full flex-col">
            {children()}
        </nav>
    }
}

#[component]
pub fn SettingsDrawerNavTop(children: Children) -> impl IntoView {
    view! { <div>{children()}</div> }
}

#[component]
pub fn SettingsDrawerNavBottom(children: Children) -> impl IntoView {
    view! { <div class="mt-auto">{children()}</div> }
}

#[component]
pub fn SettingsDrawerBrand(children: Children, href: &'static str) -> impl IntoView {
    view! {
        <A href=href attr:class="mb-5 block text-2xl font-extrabold tracking-tight text-base-content">
            {children()}
        </A>
    }
}

#[component]
pub fn SettingsNavLink(
    children: Children,
    href: &'static str,
    active: impl Fn() -> bool + Send + Sync + Clone + 'static,
    #[prop(optional)] drawer_id: Option<&'static str>,
) -> impl IntoView {
    view! {
        <li>
            <A
                href=href
                on:click=move |_| {
                    if let Some(drawer_id) = drawer_id {
                        crate::web::client::close_checkbox_drawer(drawer_id);
                    }
                }
                attr:class=move || {
                    if active() {
                        "bg-primary/10 text-primary font-semibold"
                    } else {
                        "text-base-content/70 hover:bg-base-200"
                    }
                }
            >
                {children()}
            </A>
        </li>
    }
}

#[component]
pub fn SettingsNavButton(children: Children, form: &'static str) -> impl IntoView {
    view! {
        <li>
            <button type="submit" form=form class="text-base-content/70 hover:bg-base-200">
                {children()}
            </button>
        </li>
    }
}

#[component]
pub fn SettingsSubTabs(children: Children) -> impl IntoView {
    view! { <div class="mt-5">{children()}</div> }
}

#[component]
pub fn SettingsSubTabList(children: Children) -> impl IntoView {
    view! { <div role="tablist" class="flex flex-wrap gap-3 border-b border-base-300">{children()}</div> }
}

#[component]
pub fn SettingsSubTabButton(
    children: Children,
    active: impl Fn() -> bool + Send + Sync + Clone + 'static,
    on_click: impl Fn(leptos::ev::MouseEvent) + Clone + 'static,
) -> impl IntoView {
    let active_for_class = active.clone();

    view! {
        <button
            type="button"
            role="tab"
            class=move || {
                if active_for_class() {
                    "inline-flex items-center gap-2 border-b-2 border-primary px-1 pb-3 text-sm font-semibold text-primary"
                } else {
                    "inline-flex items-center gap-2 border-b-2 border-transparent px-1 pb-3 text-sm font-semibold text-base-content/60 hover:border-base-300 hover:text-base-content"
                }
            }
            aria-selected=active
            on:click=on_click
        >
            {children()}
        </button>
    }
}

#[component]
pub fn SettingsSubTabPanel(children: Children) -> impl IntoView {
    view! { <div class="mt-4">{children()}</div> }
}

#[component]
pub fn MenuLink(children: Children, href: &'static str, active: bool) -> impl IntoView {
    view! {
        <li>
            <a href=href class:active=move || active>{children()}</a>
        </li>
    }
}

#[component]
pub fn ReactiveMenuLink(
    children: Children,
    href: &'static str,
    active: impl Fn() -> bool + Send + Sync + Clone + 'static,
) -> impl IntoView {
    view! {
        <li>
            <a href=href class=move || if active() { "active" } else { "" }>{children()}</a>
        </li>
    }
}

#[component]
pub fn MenuButton(children: Children, form: &'static str) -> impl IntoView {
    view! {
        <li>
            <button type="submit" form=form>{children()}</button>
        </li>
    }
}
