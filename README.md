# Aura MCP

Aura MCP is a local stdio MCP server for the Aura trading API.

It lets AI agents inspect Aura resources and call Aura gRPC API methods through the Rust client in [`aura_api_client`](https://github.com/in-aura-we-trade/aura_api_client).

Aura is a low-latency Solana trading platform. Through this MCP server, an AI agent can access Aura functionality such as trading, wallet management, token account management, durable nonce management, copy-trading, sniping, limit orders, live user activity, token data, processor stats, and other Aura API features.

MCP\API is powerful. Treat access to Aura MCP the same way you treat access to your trading wallet and Aura API key.

## Links

* Aura: [https://aura.rehab](https://aura.rehab)
* API endpoint: `http://trade.aura.rehab:40051`
* Telegram bot: [@trade_with_aura_bot](https://t.me/trade_with_aura_bot)
* Telegram group: [https://t.me/trade_with_aura](https://t.me/trade_with_aura)
* Aura API client: [https://github.com/in-aura-we-trade/aura_api_client](https://github.com/in-aura-we-trade/aura_api_client)
* Contact: [@saul_goodman_aura](https://t.me/saul_goodman_aura)

## What Aura MCP gives agents

Aura MCP exposes the Aura trading API to MCP-compatible AI agents.

Depending on your API key permissions and local `read_only` setting, an agent can:

* Inspect Aura docs, examples, protobuf definitions, and agent instructions.
* Query token status, token metadata, trade stats, pool data, positions, wallet state and more...
* Change wallets, snipes, copy-trade tasks, limit orders, processor stats, and DEX compute-unit settings.
* Subscribe to live `UserActivity` updates for trades, confirmations, account events, limit-order executions, and errors.
* Prepare and confirm trading operations.
* Manage wallets, token accounts, Aura utility accounts, and durable nonces.
* Create, update, enable, disable, duplicate, delete, and clear snipe tasks.
* Create, update, enable, disable, duplicate, delete, and clear copy-trade tasks.
* Place, delete, clear, and manage limit orders.
* Execute other supported Aura API mutations through confirmation-based MCP tools.

Keep `read_only = true` unless you intentionally want the connected agent to be able to prepare and confirm mutating trading actions.

## Install

Install directly from GitHub:

```bash
cargo install --git https://github.com/in-aura-we-trade/aura_mcp
```

Verify the installation:

```bash
aura-mcp --help
```

## Build from source

For local development:

```bash
git clone https://github.com/in-aura-we-trade/aura_mcp
cd aura_mcp
cargo build --release
```

The release binary will be available at:

```bash
./target/release/aura-mcp
```

## Login

Get an API key from [@trade_with_aura_bot](https://t.me/trade_with_aura_bot) in the `API | Extension` tab.

Then log in locally:

```bash
aura-mcp login --api-key <KEY>
```

By default, config is stored at:

```text
~/.config/aura/mcp.toml
```

You can override the config path with:

```bash
export AURA_MCP_CONFIG=/path/to/mcp.toml
```

Example config:

```toml
api_endpoint = "http://trade.aura.rehab:40051"
api_key = "..."
read_only = true
```

Use `read_only = true` when you want agents to inspect data without being able to execute mutations.

Set `read_only = false` only when you intentionally want the connected AI agent to have trading and management access through Aura API.

## Run

Start the MCP server:

```bash
aura-mcp serve
```

`serve` uses MCP over stdio.

Stdout is reserved for JSON-RPC messages. Logs are written to stderr.

## Client configuration

Aura MCP can print ready-to-use config snippets for supported clients.

### Claude

```bash
aura-mcp print-config claude
```

### Codex

```bash
aura-mcp print-config codex
```

Use the printed config in your MCP-compatible client.

## Resources

Aura MCP exposes resources that agents can read without calling trading tools:

```text
aura://docs/overview
aura://docs/auth
aura://docs/grpc
aura://docs/tools
aura://docs/api
aura://instructions/agent
aura://user_activity/latest
aura://proto/main
aura://examples/rust
aura://examples/typescript
```

`aura://user_activity/latest` is updated from the active user activity stream when streaming is enabled.

## Tools

Aura MCP exposes three main tool groups.

### Read-only tools

Read-only tools inspect Aura state, docs, and user data without preparing mutations.

Examples include:

```text
get_aura_status
get_account_info
list_wallets
list_snipe_tasks
list_limit_orders
get_bot_status
explain_aura_error
```

Aura MCP also exposes one-shot read methods from the Aura Rust client for token data, positions, limit orders, snipe tasks, copy-trade tasks, processor stats, DEX compute-unit settings, and other supported API surfaces.

### Streaming tools

Aura MCP owns the single Aura `UserActivity` stream for the configured API key.

Streaming tools include:

```text
start_user_activity
read_user_activity
user_activity_status
stop_user_activity
```

The MCP server sends internal `user_ping` keepalives.

Clients can poll with `read_user_activity` or subscribe to `aura://user_activity/latest` and re-read it after `notifications/resources/updated`.

### Mutating tools

Mutating tools are exposed as `prepare_*` tools.

They do not execute immediately. They prepare a mutation that must later be confirmed through:

```text
confirm_mutation
```

or a matching confirm alias such as:

```text
confirm_limit_order
confirm_snipe_task
```

Mutation tools are blocked when:

```toml
read_only = true
```

Set `read_only = false` only when you intentionally want the connected agent to be able to prepare and confirm mutations.

## Raw request tools

Raw prepare tools accept a `request` object matching the corresponding `aura_api_client` request type.

They also accept `request` as a JSON-encoded string for adapters that expose raw payloads as scalar strings.

Every raw tool includes metadata to help agents construct safer calls:

* `_meta.aura_rate_limits`
* `_meta.aura_trading_prerequisites`
* `_meta.aura_batching_recommendations`
* `_meta.aura_raw_request`
* `_meta.aura_argument_notes`

Tools with state-derived arguments include notes about where values must come from. For example:

* Snipe and copy-trade `id` fields must come from the matching list tool.
* `prepare_add_wallet.keypair_base58` must be a full base58-encoded Solana keypair secret, not a public wallet address.

## Batching recommendations

Agents should batch where the Aura API supports it.

Recommended patterns:

* Attach known follow-up limit orders directly to `MarketTrade.limit_orders`.
* Place multiple limit orders in one limit-order request.
* Use multi-entry `SnipeUpdate.updates`.
* Use multi-entry `CtUpdate.updates`.
* Use multi-entry `ConfigPubkeys.pubkeys`.

Batching reduces API calls and lowers the chance of hitting rate limits.

## Trading wallet requirements

After connecting or adding a wallet, the active trading wallet must have:

* All Aura utility accounts opened.
* At least 1 durable nonce.

This applies to:

* Market trades
* Limit orders
* Snipe execution
* Copy-trade execution

If a trading call fails with a missing account or nonce error, inspect wallets first:

```json
{
  "tool": "list_wallets",
  "arguments": {}
}
```

Then prepare and confirm utility account setup:

```json
{
  "tool": "prepare_open_util_accs",
  "arguments": {
    "address": "<WALLET>"
  }
}
```

Then prepare and confirm nonce creation:

```json
{
  "tool": "prepare_create_nonces",
  "arguments": {
    "request": {
      "wallet": "<WALLET>",
      "amount": 1
    }
  }
}
```

## Security notes

Aura MCP can give an AI agent full trading access to the Solana blockchain through Aura API.

Depending on your API key permissions and `read_only` setting, this can include:

* Trading
* Wallet management
* Token account management
* Durable nonce management
* Limit order management
* Copy-trade control
* Snipe control
* Other mutating trading operations

Security recommendations:

* Keep `read_only = true` unless you explicitly want agent-controlled mutations.
* Do not commit `mcp.toml`.
* Do not expose your Aura API key to untrusted agents, prompts, tools, or logs.
* Do not give agents access to unrelated shell or file tools unless you understand the risk.
* Use separate API keys for different agents and workflows when possible.
* Rotate/change your API key from the Telegram bot if it is exposed.
* Review prepared mutations before confirming them.
* Keep the `UserActivity` stream running when executing trading workflows so the agent can observe confirmations, errors, and account events.

The API key is stored locally and is never printed by `login`.

On Unix, the config writer requests `0600` file permissions.

## Rate limits

Aura rate limits API calls per key and IP:

```text
4 requests/second
60 requests/minute
```

Bursts above these limits can trigger stronger protection.

Avoid exceeding:

```text
10 requests/second
150 requests/minute
```

Large bursts can trigger a 24-hour ban.

Recommended agent behavior:

* Use about one live Aura API call every `0.5s` for broad sweeps.
* Batch requests where possible.
* Prefer local `prepare_*` tools before live confirmation calls.
* Remember that `confirm_mutation`, `confirm_limit_order`, and `confirm_snipe_task` call Aura and count against rate limits.
* Avoid repeatedly polling live API tools when `UserActivity` streaming can provide updates.

## Typical workflow

1. Install Aura MCP.
2. Get an API key from [@trade_with_aura_bot](https://t.me/trade_with_aura_bot).
3. Run `aura-mcp login --api-key <KEY>`.
4. Keep `read_only = true` for inspection-only agents.
5. Run `aura-mcp serve` from an MCP-compatible client.
6. Let the agent inspect docs and list current Aura state.
7. Set `read_only = false` only when you intentionally want the agent to prepare and confirm trading mutations.
8. Confirm mutations only after reviewing the prepared action.
