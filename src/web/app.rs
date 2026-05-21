use crate::web::components::layout::{BaseLayout, ProtectedLayout, PublicLayout};
use crate::web::components::ui::document_body_class;
use crate::web::pages;
use leptos::config::LeptosOptions;
use leptos::hydration::{AutoReload, HydrationScripts};
use leptos::prelude::*;
use leptos_meta::provide_meta_context;
use leptos_meta::{HashedStylesheet, MetaTags};
use leptos_router::{SsrMode, components::*, path};

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    view! {
        <BaseLayout>
            <Router>
                <Routes fallback=|| "Page not found.">
                    <ParentRoute path=path!("") view=PublicLayout ssr=SsrMode::InOrder>
                        <Route path=path!("") view=pages::Login ssr=SsrMode::InOrder />
                        <Route path=path!("login") view=pages::Login ssr=SsrMode::InOrder />
                        <Route path=path!("signup") view=pages::Signup ssr=SsrMode::InOrder />
                    </ParentRoute>
                    <ParentRoute path=path!("") view=ProtectedLayout ssr=SsrMode::InOrder>
                        <Route path=path!("dashboard") view=pages::Dashboard ssr=SsrMode::InOrder />
                        <Route path=path!("transactions") view=pages::Transactions ssr=SsrMode::InOrder />
                        <Route path=path!("transactions/:short_ref") view=pages::DocumentDetailsPage ssr=SsrMode::InOrder />
                        <Route path=path!("settings") view=pages::Settings ssr=SsrMode::InOrder />
                    </ParentRoute>
                </Routes>
            </Router>
        </BaseLayout>
    }
}

pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <html lang="en" data-theme="light">
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1" />
                <AutoReload options=options.clone() />
                <HydrationScripts options=options.clone() />
                <HashedStylesheet options=options.clone() id="leptos" />
                <title>"Finelor"</title>
                <link rel="icon" type="image/x-icon" href="/favicon.ico" />
                <MetaTags />
            </head>
            <body class=document_body_class()>
                <App />
            </body>
        </html>
    }
}
