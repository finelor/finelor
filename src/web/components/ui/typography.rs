use leptos::prelude::*;

#[component]
pub fn BrandTitle(children: Children) -> impl IntoView {
    view! {
        <h1 class="text-2xl font-extrabold tracking-tight text-base-content md:text-3xl">
            {children()}
        </h1>
    }
}

#[component]
pub fn Eyebrow(children: Children) -> impl IntoView {
    view! {
        <p class="text-xs font-bold uppercase tracking-widest text-base-content/45">
            {children()}
        </p>
    }
}

#[component]
pub fn CardTitle(children: Children) -> impl IntoView {
    view! {
        <h2 class="mt-2 text-xl font-bold text-base-content">{children()}</h2>
    }
}

#[component]
pub fn PanelTitle(children: Children) -> impl IntoView {
    view! {
        <p class="text-sm font-semibold text-base-content">{children()}</p>
    }
}

#[component]
pub fn HeroTitle(children: Children) -> impl IntoView {
    view! {
        <h2 class="break-words text-2xl font-bold leading-tight text-base-content md:text-3xl">
            {children()}
        </h2>
    }
}

#[component]
pub fn PageTitle(children: Children) -> impl IntoView {
    view! {
        <p class="text-xl font-bold">{children()}</p>
    }
}

#[component]
pub fn BodyText(children: Children) -> impl IntoView {
    view! { <p class="mt-2 text-sm text-base-content/65">{children()}</p> }
}

#[component]
pub fn LeadText(children: Children) -> impl IntoView {
    view! {
        <p class="mt-5 break-words text-base leading-7 text-base-content/65">
            {children()}
        </p>
    }
}

#[component]
pub fn MutedText(children: Children) -> impl IntoView {
    view! { <p class="text-sm text-base-content/65">{children()}</p> }
}

#[component]
pub fn FineText(children: Children) -> impl IntoView {
    view! { <p class="text-xs text-base-content/60">{children()}</p> }
}

#[component]
pub fn CenterFineText(children: Children) -> impl IntoView {
    view! { <p class="mt-2 text-center text-xs text-base-content/45">{children()}</p> }
}

#[component]
pub fn ModalTitle(children: Children) -> impl IntoView {
    view! {
        <h2 class="mt-4 text-lg font-extrabold text-base-content">
            {children()}
        </h2>
    }
}

#[component]
pub fn ModalInstruction(children: Children) -> impl IntoView {
    view! {
        <h2 class="mt-4 text-sm font-semibold text-base-content">
            {children()}
        </h2>
    }
}

#[component]
pub fn InlineText(children: Children) -> impl IntoView {
    view! {
        <span class="inline-flex items-center gap-2">
            {children()}
        </span>
    }
}

#[component]
pub fn SectionHeader(
    eyebrow: &'static str,
    title: &'static str,
    #[prop(optional)] help: Option<&'static str>,
) -> impl IntoView {
    view! {
        <div>
            <Eyebrow>{eyebrow}</Eyebrow>
            <CardTitle>{title}</CardTitle>
            <Show when=move || help.is_some()>
                <BodyText>{help.unwrap_or_default()}</BodyText>
            </Show>
        </div>
    }
}
