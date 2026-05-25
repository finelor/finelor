# Finelor MCP

## Overview

Finelor exposes a read-only Model Context Protocol server for AI agents at:

```text
/mcp
```

The MCP endpoint is separate from the public REST API under `/api/v1`.
MCP clients must use MCP keys, not public API keys.

Every MCP request requires:

```text
Authorization: Bearer <mcp_key>
```

MCP keys are created and managed in the web client under Settings -> Channels -> MCP.

## Transport

Finelor uses MCP Streamable HTTP. Clients send JSON-RPC messages to the single `/mcp` endpoint.

Required request headers:

```text
Authorization: Bearer <mcp_key>
Content-Type: application/json
Accept: application/json, text/event-stream
```

The first version is stateless and returns JSON responses directly.

## Client Setup

Use the Finelor application URL plus `/mcp` as the MCP server URL.

For local development:

```text
http://localhost:3000/mcp
```

For a deployed Finelor instance:

```text
https://your-finelor-host.example/mcp
```

Configure the MCP client to send the MCP key as a Bearer token. The exact configuration shape depends on the client, but the required values are:

```json
{
  "url": "http://localhost:3000/mcp",
  "headers": {
    "Authorization": "Bearer finelor_mcp_..."
  }
}
```

For clients that support named remote MCP servers, use:

```json
{
  "mcpServers": {
    "finelor": {
      "url": "http://localhost:3000/mcp",
      "headers": {
        "Authorization": "Bearer finelor_mcp_..."
      }
    }
  }
}
```

The same configuration as YAML:

```yaml
mcpServers:
  finelor:
    url: http://localhost:3000/mcp
    headers:
      Authorization: Bearer finelor_mcp_...
```

For Codex-style `config.toml`:

```toml
[mcp_servers.finelor]
url = "http://localhost:3000/mcp"
http_headers = { Authorization = "Bearer finelor_mcp_..." }
```

Or, preferably, keep the token in an environment variable:

```toml
[mcp_servers.finelor]
url = "http://localhost:3000/mcp"
bearer_token_env_var = "FINELOR_MCP_KEY"
```

Then start Codex with:

```sh
export FINELOR_MCP_KEY="finelor_mcp_..."
```

Some MCP clients require an explicit transport field, while others infer Streamable HTTP from the `url`. If your client requires one, use that client's expected spelling, for example `type: streamable-http`, `type: streamable_http`, or `transport: streamable-http`.

Do not use public API keys here. Public API keys authenticate `/api/v1/*`; MCP keys authenticate `/mcp`.

## Tools

The initial MCP surface is read-only.

Available tools:

| Tool | Capability | Description |
| --- | --- | --- |
| `document_status_summary` | `documents:read` | Returns exact document counts by processing status. |
| `list_documents` | `documents:read` | Lists recent documents with exact total count and compact items. |
| `get_document` | `documents:read` | Returns compact status/details for one document by `short_ref`. |
| `explain_document` | `documents:explain` | Explains why one document is blocked, pending, failed, or ready. |

Not exposed in this version:

- Pending review lists
- Export-ready lists
- Ingestion
- Review, approval, retry, remove, or export actions
- Source file downloads
- Raw SQL or arbitrary database access

## Tool Arguments

### document_status_summary

No arguments.

```json
{}
```

### list_documents

```json
{
  "limit": 10
}
```

Fields:

| Field | Type | Description |
| --- | --- | --- |
| `limit` | integer or null | Maximum number of recent documents to return. Clamped to `1..20`. |

### get_document

```json
{
  "short_ref": "D000123"
}
```

Fields:

| Field | Type | Description |
| --- | --- | --- |
| `short_ref` | string | Finelor document reference. Numeric shorthand may be normalized by Finelor where supported. |

### explain_document

```json
{
  "short_ref": "D000123"
}
```

Fields:

| Field | Type | Description |
| --- | --- | --- |
| `short_ref` | string | Finelor document reference. Numeric shorthand may be normalized by Finelor where supported. |

## Capabilities

MCP keys currently receive these default capabilities:

```json
["documents:read", "documents:explain"]
```

Capability editing is not exposed in the web client yet. The server still enforces capabilities internally so future MCP keys can become more granular without changing the tool boundary.

Unknown or malformed key capabilities fail closed.

## Security Notes

- MCP keys are separate from public API keys.
- Revoked or removed MCP keys cannot authenticate.
- Browser requests with invalid `Origin` values are rejected.
- The MCP endpoint does not expose write actions in the first version.
