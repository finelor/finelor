use crate::web::client::redirect;
use crate::web::components::navigation::AuthTopNav;
use crate::web::components::ui::{
    Alert, AlertKind, AppCanvas, ButtonKind, Footer, HiddenPlaceholder, LinkButton, LoadingCard,
    ProtectedFrame, ProtectedMain, PublicFrame,
};
use crate::web::server::auth::get_session_user;
use leptos::prelude::*;
use leptos_router::components::Outlet;

#[component]
pub fn BaseLayout(children: Children) -> impl IntoView {
    view! { <AppCanvas>{children()}</AppCanvas> }
}

#[component]
pub fn PublicLayout() -> impl IntoView {
    view! {
        <PublicFrame>
            <Outlet />
        </PublicFrame>
        <AppFooter wide=false />
    }
}

#[component]
pub fn ProtectedLayout() -> impl IntoView {
    let session_user = Resource::new(|| (), |_| get_session_user());

    Effect::new(move || {
        if let Some(Ok(None)) = session_user.get() {
            redirect("/login");
        }
    });

    view! {
        <Suspense fallback=|| view! {
            <ProtectedFrame>
                <AuthTopNav />
                <ProtectedMain>
                    <LoadingCard message="Checking session..." />
                </ProtectedMain>
                <AppFooter wide=true />
            </ProtectedFrame>
        }>
            {move || match session_user.get() {
                Some(Ok(Some(_))) => view! {
                    <ProtectedFrame>
                        <AuthTopNav />
                        <ProtectedMain>
                            <Outlet />
                        </ProtectedMain>
                        <AppFooter wide=true />
                    </ProtectedFrame>
                }.into_any(),
                Some(Ok(None)) => view! {
                    <ProtectedFrame>
                        <AuthTopNav />
                        <ProtectedMain>
                            <Alert kind=AlertKind::Error>
                                <span>
                                    "You need to sign in to view this page. "
                                    <LinkButton href="/login".to_string() kind=ButtonKind::Ghost>
                                        "Go to login"
                                    </LinkButton>
                                </span>
                            </Alert>
                        </ProtectedMain>
                        <AppFooter wide=true />
                    </ProtectedFrame>
                }.into_any(),
                Some(Err(err)) => view! {
                    <ProtectedFrame>
                        <AuthTopNav />
                        <ProtectedMain>
                            <Alert kind=AlertKind::Error>
                                {format!("Failed to check session: {}", err)}
                            </Alert>
                        </ProtectedMain>
                        <AppFooter wide=true />
                    </ProtectedFrame>
                }.into_any(),
                None => view! { <HiddenPlaceholder /> }.into_any(),
            }}
        </Suspense>
    }
}

#[component]
fn AppFooter(wide: bool) -> impl IntoView {
    view! {
        <Footer wide=wide>
            <p>"© 2026 Finelor. Your accounting department, on autopilot."</p>
        </Footer>
    }
}
