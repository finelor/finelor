#[cfg(feature = "ssr")]
pub mod agents;
#[cfg(feature = "ssr")]
pub mod api;
#[cfg(feature = "ssr")]
pub mod confidence;
#[cfg(feature = "ssr")]
pub mod config;
#[cfg(feature = "ssr")]
pub mod db;
#[cfg(feature = "ssr")]
pub mod error;
#[cfg(feature = "ssr")]
pub mod export;
#[cfg(feature = "ssr")]
pub mod inference;
#[cfg(feature = "ssr")]
pub mod ingestion;
#[cfg(feature = "ssr")]
pub mod integrations;
#[cfg(feature = "ssr")]
pub mod invoice;
#[cfg(feature = "ssr")]
pub mod kv;
#[cfg(feature = "ssr")]
pub mod mcp;
#[cfg(feature = "ssr")]
pub mod messaging;
#[cfg(feature = "ssr")]
pub mod orchestration;
#[cfg(feature = "ssr")]
pub mod query;
#[cfg(feature = "ssr")]
pub mod queue;
#[cfg(feature = "ssr")]
pub mod security;
#[cfg(feature = "ssr")]
pub mod skills;
pub mod web;
#[cfg(feature = "ssr")]
pub mod workspace;
#[cfg(feature = "ssr")]
pub use integrations::telegram;

#[cfg(feature = "hydrate")]
#[leptos::wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(web::app::App);
}
