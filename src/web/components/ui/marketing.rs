use icondata::Icon as IconData;
use leptos::prelude::*;
use leptos_icons::Icon;

#[component]
pub fn FeatureList(children: Children) -> impl IntoView {
    view! {
        <div class="grid gap-3">
            {children()}
        </div>
    }
}

#[component]
pub fn FeatureItem(children: Children, icon: IconData) -> impl IntoView {
    view! {
        <div class="flex items-center gap-3 text-sm text-base-content/70">
            <span class="flex size-7 shrink-0 items-center justify-center rounded-box bg-info/10 text-info">
                <Icon icon=icon width="1rem" height="1rem" />
            </span>
            <span>{children()}</span>
        </div>
    }
}

#[component]
pub fn MarketingProofStack(children: Children) -> impl IntoView {
    view! {
        <div class="space-y-8">
            {children()}
        </div>
    }
}
