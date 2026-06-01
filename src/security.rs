use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::Response,
};

use crate::config::WebConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Authority {
    host: String,
    port: Option<u16>,
}

pub async fn validate_host(
    State(config): State<WebConfig>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    if request.uri().path() == "/health" {
        return Ok(next.run(request).await);
    }

    if host_is_allowed(request.headers(), &config.allowed_hosts) {
        Ok(next.run(request).await)
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

pub async fn validate_origin(
    State(config): State<WebConfig>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    if origin_is_allowed(request.headers(), &config.allowed_origins) {
        Ok(next.run(request).await)
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

fn host_is_allowed(headers: &HeaderMap, allowed_hosts: &[String]) -> bool {
    if allows_any(allowed_hosts) {
        return true;
    }

    let Some(requested) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_authority)
    else {
        return false;
    };

    allowed_hosts
        .iter()
        .filter_map(|value| parse_authority(value))
        .any(|allowed| {
            allowed.host == requested.host
                && allowed
                    .port
                    .map(|port| Some(port) == requested.port)
                    .unwrap_or(true)
        })
}

fn origin_is_allowed(headers: &HeaderMap, allowed_origins: &[String]) -> bool {
    if allows_any(allowed_origins) {
        return true;
    }

    let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(normalize_origin)
    else {
        return true;
    };

    allowed_origins
        .iter()
        .map(|value| normalize_origin(value))
        .any(|allowed| allowed == origin)
}

fn allows_any(values: &[String]) -> bool {
    values.iter().any(|value| value.trim() == "*")
}

fn normalize_origin(value: &str) -> String {
    value.trim().trim_end_matches('/').to_ascii_lowercase()
}

fn parse_authority(value: &str) -> Option<Authority> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() {
        return None;
    }

    if let Some(rest) = value.strip_prefix('[') {
        let (host, after_host) = rest.split_once(']')?;
        let port = after_host.strip_prefix(':').and_then(parse_port);
        return Some(Authority {
            host: host.to_string(),
            port,
        });
    }

    if let Some((host, port)) = value.rsplit_once(':')
        && !host.contains(':')
    {
        return Some(Authority {
            host: host.to_string(),
            port: parse_port(port),
        });
    }

    Some(Authority {
        host: value,
        port: None,
    })
}

fn parse_port(value: &str) -> Option<u16> {
    value.parse::<u16>().ok()
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header};

    use super::{host_is_allowed, origin_is_allowed};

    #[test]
    fn host_allowlist_matches_host_without_requiring_port() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::HOST,
            HeaderValue::from_static("finelor.example:443"),
        );

        assert!(host_is_allowed(&headers, &["finelor.example".to_string()]));
    }

    #[test]
    fn host_allowlist_rejects_unlisted_host() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("other.example"));

        assert!(!host_is_allowed(&headers, &["finelor.example".to_string()]));
    }

    #[test]
    fn host_allowlist_allows_wildcard() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("203.0.113.10:3000"));

        assert!(host_is_allowed(&headers, &["*".to_string()]));
    }

    #[test]
    fn origin_allowlist_allows_missing_origin() {
        let headers = HeaderMap::new();

        assert!(origin_is_allowed(
            &headers,
            &["https://finelor.example".to_string()]
        ));
    }

    #[test]
    fn origin_allowlist_rejects_unlisted_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://other.example"),
        );

        assert!(!origin_is_allowed(
            &headers,
            &["https://finelor.example".to_string()]
        ));
    }

    #[test]
    fn origin_allowlist_allows_wildcard() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://203.0.113.10:3000"),
        );

        assert!(origin_is_allowed(&headers, &["*".to_string()]));
    }
}
