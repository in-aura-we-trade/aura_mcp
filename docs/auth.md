# Aura Auth

Aura API keys are supplied as gRPC metadata with the `auth` key. This MCP server reuses the `UserCtx` auth logic from `aura_api_client`.

Local config defaults to:

```toml
api_endpoint = "http://trade.aura.rehab:40051"
api_key = "..."
read_only = true
```

The default path is `~/.config/aura/mcp.toml`. Override it with `AURA_MCP_CONFIG=/custom/path/mcp.toml`.

Use `aura-mcp login --api-key <KEY>` to create or update the file. The command does not print the API key back.
