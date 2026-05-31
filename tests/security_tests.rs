use axum::{Router, routing::get};
use tower::util::ServiceExt;

fn web_config() -> finelor::config::WebConfig {
    finelor::config::WebConfig {
        allowed_hosts: vec!["finelor.example".to_string()],
        allowed_origins: vec!["https://finelor.example".to_string()],
    }
}

fn guarded_app() -> Router {
    let config = web_config();
    Router::new()
        .route(
            "/_server_fn/test",
            get(|| async { "ok" }).route_layer(axum::middleware::from_fn_with_state(
                config.clone(),
                finelor::security::validate_origin,
            )),
        )
        .route("/api/v1/documents", get(|| async { "ok" }))
        .route("/health", get(|| async { "ok" }))
        .layer(axum::middleware::from_fn_with_state(
            config,
            finelor::security::validate_host,
        ))
}

async fn request_get(path: &str, host: &str, origin: Option<&str>) -> axum::response::Response {
    let mut request = axum::http::Request::builder()
        .method("GET")
        .uri(path)
        .header("Host", host);
    if let Some(origin) = origin {
        request = request.header("Origin", origin);
    }

    guarded_app()
        .oneshot(request.body(axum::body::Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn server_fn_route_accepts_configured_origin() {
    let response = request_get(
        "/_server_fn/test",
        "finelor.example",
        Some("https://finelor.example"),
    )
    .await;

    assert_eq!(response.status(), axum::http::StatusCode::OK);
}

#[tokio::test]
async fn server_fn_route_rejects_invalid_origin() {
    let response = request_get(
        "/_server_fn/test",
        "finelor.example",
        Some("https://other.example"),
    )
    .await;

    assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn server_fn_route_rejects_invalid_host() {
    let response = request_get(
        "/_server_fn/test",
        "other.example",
        Some("https://finelor.example"),
    )
    .await;

    assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_route_is_not_same_origin_restricted() {
    let response = request_get(
        "/api/v1/documents",
        "finelor.example",
        Some("https://other.example"),
    )
    .await;

    assert_eq!(response.status(), axum::http::StatusCode::OK);
}

#[tokio::test]
async fn health_route_is_not_same_origin_restricted() {
    let response = request_get("/health", "finelor.example", Some("https://other.example")).await;

    assert_eq!(response.status(), axum::http::StatusCode::OK);
}

#[tokio::test]
async fn health_route_is_not_host_restricted() {
    let response = request_get("/health", "localhost", None).await;

    assert_eq!(response.status(), axum::http::StatusCode::OK);
}
