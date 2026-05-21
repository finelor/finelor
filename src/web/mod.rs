pub mod app;
pub mod client;
pub mod components;
pub mod events;
pub mod pages;
pub mod server;

#[cfg(feature = "ssr")]
pub mod pool;

#[cfg(feature = "ssr")]
use axum::{Router, extract::FromRef};
#[cfg(feature = "ssr")]
use leptos::config::LeptosOptions;
#[cfg(feature = "ssr")]
use leptos_axum::{LeptosRoutes, generate_route_list, render_app_to_stream_in_order};

#[cfg(feature = "ssr")]
pub fn mount_web_router<S>(router: Router<S>, state: &S) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
    LeptosOptions: FromRef<S>,
{
    let routes = generate_route_list(app::App);
    let leptos_options = LeptosOptions::from_ref(state);

    router
        .leptos_routes(state, routes, {
            let options = leptos_options.clone();
            move || app::shell(options.clone())
        })
        .fallback(render_app_to_stream_in_order({
            let options = leptos_options.clone();
            move || app::shell(options.clone())
        }))
}
