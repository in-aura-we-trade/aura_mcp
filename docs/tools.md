# Aura MCP Tools

Read-only tools:

- `get_aura_status`
- `get_account_info`
- `list_wallets`
- `list_snipe_tasks`
- `list_limit_orders`
- `get_bot_status`
- `explain_aura_error`
- `start_user_activity`
- `read_user_activity`
- `user_activity_status`
- `stop_user_activity`

Mutation tools:

- `prepare_limit_order` / `prepare_place_limit_orders`
- `cancel_limit_order` / `prepare_delete_limit_orders`
- `prepare_trade`
- `prepare_snipe_*`
- `prepare_ct_*`
- `prepare_*` utility tools such as `prepare_switch_wallet`, `prepare_wrap_wsol`, and `prepare_dex_cu_set`
- `confirm_limit_order`, `confirm_snipe_task`, and `confirm_mutation`

When `read_only = true`, mutation tools return an error and do not call Aura. Confirmations are stored in memory and expire after five minutes.

Raw prepare tools accept a `request` object matching the corresponding Rust client request type from `aura_api_client`. They also accept `request` as a JSON-encoded string for tool adapters that cannot send object-valued raw payloads. The same raw request object can be passed directly, but wrapped `request` payloads are preferred because they are unambiguous.
For MCP JSON, provide Solana `Address` values as base58 strings; the server converts them to the typed client values before calling Aura.

Every raw tool schema has a top-level JSON Schema `type: "object"` for OpenAI/Codex function adapters. Inside that object, raw tools accept both object and JSON-string forms, and every raw tool includes `_meta.aura_raw_request` with accepted forms plus examples for high-frequency workflows.

Every tool may include `_meta.aura_argument_notes` for fields that need values from prior Aura state. In particular, snipe `id` values must come from `snipe_get_cfgs` or `list_snipe_tasks`, copy-trade `id` values must come from `ct_get_cfgs`, and confirmation tools must use `data.confirmation_id` from a prepare response.

`prepare_add_wallet.keypair_base58` is a full Solana keypair secret encoded as base58, usually the 64 secret-key bytes encoded with bs58. It is not a wallet address/public key, and arbitrary 64-byte data is not necessarily a valid Solana keypair.

Every tool description includes the Aura rate limits. Every tool also includes `_meta.aura_rate_limits` with numeric limits so agents can read them programmatically. Trading-related tools include `_meta.aura_trading_prerequisites` and repeat the account setup requirement in the description.

Every tool also includes `_meta.aura_batching_recommendations`. Agents should batch actions when possible: pass follow-up orders through `MarketTrade.limit_orders`, place multiple limit orders in one `UpdateTokenLimitOrders.orders` payload, send multiple `SnipeUpdate.updates` or `CtUpdate.updates` entries in one task edit, and update multiple pubkeys through one `ConfigPubkeys.pubkeys` list.

For `prepare_trade`, `prepare_place_limit_orders`, and `prepare_limit_order`, agents can omit common execution settings. MCP fills friendly defaults from `aura_api_client`: slippage, buy/sell tip, buy/sell priority fee, `TxnProcessors::default()`, durable nonce, default price impact, empty trade filters, and a 16-slot latency. Required intent fields such as mint, amount, target, and wallet still need to be supplied.

Percent `UD128` values are ratios: `0` is 0%, `"1"` / `UD128::ONE` is 100%, and `"0.5"` / `udec128!(0.5)` is 50%. Do not pass `100` for 100%.

## Trading Prerequisites

After wallet connection, the active wallet must have all Aura utility accounts opened and at least 1 durable nonce. This is required before market trades, executable limit orders, snipe execution, and copy-trade execution.

If a trading API call fails with a missing account or nonce error:

1. Call `list_wallets`.
2. Check `accounts_state.token_accounts` and `accounts_state.util_accs.durable_nonces`.
3. Prepare and confirm `prepare_open_util_accs` with the wallet address.
4. Prepare and confirm `prepare_create_nonces` with `amount = 1`.

Example MCP calls:

```json
{"name": "prepare_open_util_accs", "arguments": {"address": "<WALLET>"}}
```

```json
{"name": "prepare_create_nonces", "arguments": {"request": {"wallet": "<WALLET>", "amount": 1}}}
```

Example token mint for token and pool read tools:

`EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v` (USDC)

## Rate Limits

The Aura API currently allows 4 requests per second and 60 requests per minute per API key and per IP address. Bursts above 10 requests per second or 150 requests per minute can trigger a 24-hour ban.

Agents should pace live tool execution. Preparing a mutation is local; confirming a mutation calls the Aura API and counts against the rate limit.

The recommended broad-test cadence is one live Aura API call every 0.5 s.

## User Activity Stream

`start_user_activity` starts one internal Aura `user_activity` stream for the configured API key. Calling it again is idempotent and returns the existing stream status.

The server sends `user_ping` internally while the stream is active. Agents should use `get_aura_status` for health checks and should not manually keep the activity stream alive with direct `aura_user_ping` calls.

`read_user_activity` accepts:

```json
{
  "after_seq": 0,
  "limit": 100
}
```

It lazily starts the stream if needed, clamps `limit` to 500, and returns buffered events with `seq > after_seq` without draining them.

`aura://user_activity/latest` returns a JSON snapshot containing the latest event, stream status, sequence, buffered count, dropped count, and latest ping/event timestamps. MCP clients can subscribe to this resource; on each new event the server emits `notifications/resources/updated` with only the URI, and clients can re-read the resource or call `read_user_activity`.

## Payload Notes

`prepare_dex_cu_set` accepts the full `DexCu` client shape. Passing an empty object uses `DexCu::init()` from `aura_api_client`, which currently maps to these client defaults:

```json
{
  "request": {
    "pump_buy": 115000,
    "pump_sell": 95000,
    "pump_amm_buy": 150000,
    "pump_amm_sell": 140000,
    "ray_amm_buy": 49000,
    "ray_amm_sell": 47000,
    "ray_cpmm_buy": 70000,
    "ray_cpmm_sell": 57000,
    "ray_ll_buy": 130000,
    "ray_ll_sell": 92000
  }
}
```

Equivalent default call:

```json
{}
```
