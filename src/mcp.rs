use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::Response,
};
use rmcp::{
    ErrorData as McpError, Json, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    config::WebConfig,
    db::{self, DbPool, McpKey},
    messaging::tools::{ReadOnlyToolCall, execute_read_only_tool},
};

const CAP_DOCUMENTS_READ: &str = "documents:read";
const CAP_DOCUMENTS_EXPLAIN: &str = "documents:explain";

#[derive(Debug, Clone)]
pub struct McpKeyContext {
    pub id: i64,
    pub capabilities: Vec<String>,
}

impl McpKeyContext {
    fn from_key(key: McpKey) -> Self {
        Self {
            id: key.id,
            capabilities: parse_capabilities(&key.capabilities),
        }
    }

    fn has_capability(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|value| value == capability)
    }
}

#[derive(Debug, Clone)]
pub struct FinelorMcpServer {
    pool: DbPool,
    tool_router: ToolRouter<Self>,
}

impl FinelorMcpServer {
    pub fn new(pool: DbPool) -> Self {
        Self {
            pool,
            tool_router: Self::tool_router(),
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListDocumentsArgs {
    #[schemars(description = "Maximum number of recent documents to return, clamped to 1..20.")]
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DocumentRefArgs {
    #[schemars(description = "Finelor document reference, for example D000123.")]
    pub document_short_ref: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct McpToolResponse {
    pub ok: bool,
    pub tool: String,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

#[tool_router(router = tool_router)]
impl FinelorMcpServer {
    #[tool(description = "Get exact Finelor document counts by processing status.")]
    pub async fn document_status_summary(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<McpToolResponse>, McpError> {
        self.execute_tool(
            mcp_key_from_context(&context)?,
            CAP_DOCUMENTS_READ,
            ReadOnlyToolCall {
                name: "document_status_summary".to_string(),
                args: json!({}),
            },
        )
        .await
    }

    #[tool(description = "List recent Finelor documents with exact total_count and compact items.")]
    pub async fn list_documents(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<ListDocumentsArgs>,
    ) -> Result<Json<McpToolResponse>, McpError> {
        self.execute_tool(
            mcp_key_from_context(&context)?,
            CAP_DOCUMENTS_READ,
            ReadOnlyToolCall {
                name: "list_documents".to_string(),
                args: json!({ "limit": args.limit }),
            },
        )
        .await
    }

    #[tool(description = "Get compact status and document details by document_short_ref.")]
    pub async fn get_document(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<DocumentRefArgs>,
    ) -> Result<Json<McpToolResponse>, McpError> {
        self.execute_tool(
            mcp_key_from_context(&context)?,
            CAP_DOCUMENTS_READ,
            ReadOnlyToolCall {
                name: "get_document".to_string(),
                args: json!({ "document_short_ref": args.document_short_ref }),
            },
        )
        .await
    }

    #[tool(description = "Explain why one Finelor document is blocked, pending, failed, or ready.")]
    pub async fn explain_document(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(args): Parameters<DocumentRefArgs>,
    ) -> Result<Json<McpToolResponse>, McpError> {
        self.execute_tool(
            mcp_key_from_context(&context)?,
            CAP_DOCUMENTS_EXPLAIN,
            ReadOnlyToolCall {
                name: "explain_document".to_string(),
                args: json!({ "document_short_ref": args.document_short_ref }),
            },
        )
        .await
    }

    async fn execute_tool(
        &self,
        key: McpKeyContext,
        capability: &'static str,
        tool_call: ReadOnlyToolCall,
    ) -> Result<Json<McpToolResponse>, McpError> {
        if !key.has_capability(capability) {
            return Err(McpError::invalid_params(
                format!("MCP key does not include required capability: {capability}"),
                None,
            ));
        }

        let value = execute_read_only_tool(&self.pool, None, &tool_call).await;
        Ok(Json(McpToolResponse {
            ok: value
                .get("ok")
                .and_then(|value| value.as_bool())
                .unwrap_or(false),
            tool: value
                .get("tool")
                .and_then(|value| value.as_str())
                .unwrap_or(tool_call.name.as_str())
                .to_string(),
            result: value.get("result").cloned(),
            error: value
                .get("error")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned),
        }))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for FinelorMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Read-only Finelor MCP server for document status and details.")
    }
}

pub fn router(pool: DbPool, web_config: WebConfig) -> Router {
    let service_pool = pool.clone();
    let service = StreamableHttpService::new(
        move || Ok(FinelorMcpServer::new(service_pool.clone())),
        std::sync::Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_allowed_hosts(web_config.allowed_hosts)
            .disable_allowed_origins()
            .with_stateful_mode(false)
            .with_json_response(true),
    );

    Router::new()
        .nest_service("/mcp", service)
        .route_layer(middleware::from_fn_with_state(
            pool,
            authenticate_mcp_request,
        ))
}

async fn authenticate_mcp_request(
    State(pool): State<DbPool>,
    mut request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let token = bearer_token(request.headers()).ok_or(StatusCode::UNAUTHORIZED)?;
    let key = db::authenticate_mcp_key(&pool, token)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    request
        .extensions_mut()
        .insert(McpKeyContext::from_key(key));
    Ok(next.run(request).await)
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn parse_capabilities(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

fn mcp_key_from_context(context: &RequestContext<RoleServer>) -> Result<McpKeyContext, McpError> {
    let parts = context
        .extensions
        .get::<axum::http::request::Parts>()
        .ok_or_else(|| McpError::invalid_params("missing HTTP request context", None))?;
    parts
        .extensions
        .get::<McpKeyContext>()
        .cloned()
        .ok_or_else(|| McpError::invalid_params("missing MCP key context", None))
}
