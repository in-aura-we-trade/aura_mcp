# Aura MCP

Local stdio MCP server for the Aura trading API. It lets AI agents make  toolcalls to the Aura gRPC API and inspect Aura docs/resources through the Rust client in `https://github.com/in-aura-we-trade/aura_api_client`.

Aura MCP gives AI agents full control over Aura API functionality. Depending on your API key permissions and `read_only` setting, an agent can trade on Solana, manage token accounts, manage wallets, manage durable nonces, create and control copy-trade tasks, create and control snipe tasks, manage limit orders, inspect live user activity, and access the full Aura trading API.

This is powerful. Treat access to this MCP server the same way you treat access to your trading wallet/API key.

## Links

- Aura: https://aura.rehab
- API: `http://trade.aura.rehab:40051`
- Telegram bot: https://t.me/trade_with_aura_bot
- Telegram group: https://t.me/trade_with_aura

## Install

Install directly from GitHub:

```bash
cargo install --git https://github.com/in-aura-we-trade/aura_mcp
````

Then verify:

```bash
aura-mcp --help
```

## Build

For local development:

```bash
cargo build --release
```

The release binary will be available at:

```bash
./target/release/aura-mcp
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

Use `read_only = true` if you want agents to inspect data without being able to execute mutations.

Set `read_only = false` only when you intentionally want the connected AI agent to have trading and management access through Aura API.

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

* `aura://docs/overview`
* `aura://docs/auth`
* `aura://docs/grpc`
* `aura://docs/tools`
* `aura://user_activity/latest`
* `aura://proto/main`
* `aura://examples/rust`
* `aura://examples/typescript`

## Tools

Read-only: `get_aura_status`, `get_account_info`, `list_wallets`, `list_snipe_tasks`, `list_limit_orders`, `get_bot_status`, `explain_aura_error`, plus one-shot read methods from the Aura Rust client for token data, positions, limit orders, snipe tasks, copy-trade tasks, processor stats, and DEX CU settings.

Streaming activity: `start_user_activity`, `read_user_activity`, `user_activity_status`, and `stop_user_activity`. The MCP server owns the single Aura `user_activity` stream for the configured API key and sends internal `user_ping` keepalives. Clients can poll with `read_user_activity` or subscribe to `aura://user_activity/latest` and re-read it after `notifications/resources/updated`.

Mutating: all non-streaming mutating Rust client calls are exposed through `prepare_*` tools and execute only through `confirm_mutation` or a matching confirm alias. Raw prepare tools accept a `request` object matching the corresponding `aura_api_client` request type.

Mutation tools require confirmation and are blocked when `read_only = true`.

## Security Notes

The API key is stored locally and is never printed by `login`. On Unix, the config writer requests `0600` file permissions.

Do not give AI agents access to unrelated shell or file tools through this MCP server.

Aura MCP can provide an AI agent with full trading access to the Solana blockchain through Aura API, including trading, wallet management, token account management, durable nonce management, copy-trade control, snipe control, and other mutating trading operations.

Keep `read_only = true` unless you explicitly want the connected agent to be able to prepare and confirm mutations.

## Rate Limits

Aura rate limits API calls per key and IP: 4 requests/second and 60 requests/minute. Bursts above 10 requests/second or 150 requests/minute can trigger a 24-hour ban. MCP clients should throttle live tool calls, especially confirmed mutations.
