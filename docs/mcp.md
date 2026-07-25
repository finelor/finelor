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

## Using Finelor MCP from OpenClaw and Hermes

Finelor's MCP endpoint can be consumed directly by remote MCP clients such as OpenClaw and Hermes.

Prerequisites:

- Finelor must be running at a reachable URL, for example `https://finelor.example.com/mcp`
- You need an MCP key created in Finelor Settings
- The MCP client must be able to reach the Finelor host

### OpenClaw

OpenClaw can connect to Finelor as a remote MCP server over Streamable HTTP.

Add Finelor with:

```bash
openclaw mcp set finelor '{
  "url": "https://finelor.example.com/mcp",
  "transport": "streamable-http",
  "headers": {
    "Authorization": "Bearer <YOUR_FINELOR_MCP_KEY>"
  }
}'
```

Check the saved config:

```bash
openclaw mcp show finelor
```

Notes:

- Use the public Finelor `/mcp` endpoint.
- Replace `<YOUR_FINELOR_MCP_KEY>` with a real MCP key from Finelor.
- OpenClaw-managed MCP definitions are saved in OpenClaw config and used by OpenClaw tool-enabled runtimes.

### Hermes

Hermes can connect to Finelor as a remote HTTP MCP server.

Add Finelor to `~/.hermes/config.yaml`:

```yaml
mcp_servers:
  finelor:
    url: "https://finelor.example.com/mcp"
    headers:
      Authorization: "Bearer <YOUR_FINELOR_MCP_KEY>"
    tools:
      include:
        - document_status_summary
        - list_documents
        - get_document
        - explain_document
```

Then reload MCP servers in Hermes:

```text
/reload-mcp
```

Notes:

- Hermes usually selects MCP tools automatically during normal reasoning.
- If you restrict tools with `include`, keep the list aligned with the tools Finelor currently exposes.

### Example chats

Once Finelor MCP is configured, you can ask things like:

- “Show me the current Finelor document status summary.”
- “List the 10 most recent Finelor documents.”
- “Explain why document D000123 is blocked.”
- “Get the details for document D000123 and summarize its intake and accounting status.”
- “Which Finelor documents are still in intake processing, and which are ready for export?”

Example follow-up prompts:

- “List recent Finelor documents and point out any that need human review.”
- “Explain the current intake and accounting status of D000123 in plain language.”
- “Compare the last five Finelor documents and tell me which ones are blocked or failed.”
- “Summarize what is waiting in Finelor right now so I know what to review first.”

### Troubleshooting

- If the client cannot connect, verify that the Finelor `/mcp` endpoint is reachable from that machine.
- If authentication fails, generate a new MCP key in Finelor and update the bearer token in the client config.
- If no tools appear, reload MCP configuration in the client and confirm that Finelor is exposing `/mcp`.
- If Finelor is behind host or origin restrictions, make sure the deployment is configured to allow the hostname you are using.

Do not use public API keys here. Public API keys authenticate `/api/v1/*`; MCP keys authenticate `/mcp`.

## Tools

The initial MCP surface is read-only.

Available tools:

| Tool | Capability | Description |
| --- | --- | --- |
| `document_status_summary` | `documents:read` | Returns exact document counts grouped by intake and accounting domains. |
| `list_documents` | `documents:read` | Lists recent documents with exact total count and nested domain status summaries. |
| `get_document` | `documents:read` | Returns compact document details plus nested domain status detail for one document by `document_short_ref`. |
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
  "document_short_ref": "D000123"
}
```

Fields:

| Field | Type | Description |
| --- | --- | --- |
| `document_short_ref` | string | Finelor document reference. Numeric shorthand may be normalized by Finelor where supported. |

### explain_document

```json
{
  "document_short_ref": "D000123"
}
```

Fields:

| Field | Type | Description |
| --- | --- | --- |
| `document_short_ref` | string | Finelor document reference. Numeric shorthand may be normalized by Finelor where supported. |

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
- MCP can be called from any client location when the request uses an allowed Finelor host and a valid MCP key.
- MCP does not use the web client's same-origin restriction.
- The MCP endpoint does not expose write actions in the first version.
