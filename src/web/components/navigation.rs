use crate::web::components::ui::{
    AppBar, AppBarDrawerButton, AppBarEnd, AppBarStart, AppBarStartCluster, AvatarLink, BrandLink,
    SETTINGS_DRAWER_ID,
};
use icondata::LuMenu;
use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_router::hooks::use_location;

#[component]
pub fn AuthTopNav() -> impl IntoView {
    let location = use_location();
    let show_settings_drawer_button =
        move || location.pathname.get().trim_start_matches('/') == "settings";

    view! {
        <AppBar>
            <AppBarStart>
                <AppBarStartCluster>
                    <Show when=show_settings_drawer_button>
                        <AppBarDrawerButton
                            drawer_id=SETTINGS_DRAWER_ID
                            label="Open settings navigation"
                        >
                            <Icon icon=LuMenu width="1.25rem" height="1.25rem" />
                        </AppBarDrawerButton>
                    </Show>
                    <BrandLink href="/dashboard">"Finelor."</BrandLink>
                </AppBarStartCluster>
            </AppBarStart>
            <AppBarEnd>
                <AvatarLink href="/settings" label="Open profile and settings">
                    "P"
                </AvatarLink>
            </AppBarEnd>
        </AppBar>
    }
}
