# Aura MCP

Local stdio MCP server for the Aura trading API. It lets AI agents inspect Aura docs/resources and call Aura gRPC API through the Rust client in `https://github.com/in-aura-we-trade/aura_api_client`.

## Build

```bash
cargo build --release
```

## Login

```bash
aura-mcp login --api-key <KEY>
```

Config defaults to `~/.config/aura/mcp.toml` and can be overridden with `AURA_MCP_CONFIG`.

```toml
api_endpoint = "http://trade.aura.rehab:40051"
api_key = "..."
read_only = true
```

## Run

```bash
aura-mcp serve
```

`serve` uses MCP over stdio. Stdout is reserved for JSON-RPC messages; logs go to stderr.

## Claude Config

```bash
aura-mcp print-config claude
```

## Codex Config

```bash
aura-mcp print-config codex
```

## Resources

- `aura://docs/overview`
- `aura://docs/auth`
- `aura://docs/grpc`
- `aura://docs/tools`
- `aura://user_activity/latest`
- `aura://proto/main`
- `aura://examples/rust`
- `aura://examples/typescript`

## Tools

Read-only: `get_aura_status`, `get_account_info`, `list_wallets`, `list_snipe_tasks`, `list_limit_orders`, `get_bot_status`, `explain_aura_error`, plus one-shot read methods from the Aura Rust client for token data, positions, limit orders, snipe tasks, copy-trade tasks, processor stats, and DEX CU settings.

Streaming activity: `start_user_activity`, `read_user_activity`, `user_activity_status`, and `stop_user_activity`. The MCP server owns the single Aura `user_activity` stream for the configured API key and sends internal `user_ping` keepalives. Clients can poll with `read_user_activity` or subscribe to `aura://user_activity/latest` and re-read it after `notifications/resources/updated`.

Mutating: all non-streaming mutating Rust client calls are exposed through `prepare_*` tools and execute only through `confirm_mutation` or a matching confirm alias. Raw prepare tools accept a `request` object matching the corresponding `aura_api_client` request type.

Mutation tools require confirmation and are blocked when `read_only = true`.

## Security Notes

The API key is stored locally and is never printed by `login`. On Unix, the config writer requests `0600` file permissions. Do not give AI agents access to unrelated shell or file tools through this MCP server.

## Rate Limits

Aura rate limits API calls per key and IP: 4 requests/second and 60 requests/minute. Bursts above 10 requests/second or 150 requests/minute can trigger a 24-hour ban. MCP clients should throttle live tool calls, especially confirmed mutations.
