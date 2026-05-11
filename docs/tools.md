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

Raw prepare tools accept a `request` object matching the corresponding Rust client request type from `aura_api_client`.
For MCP JSON, provide Solana `Address` values as base58 strings; the server converts them to the typed client values before calling Aura.

Example token mint for token and pool read tools:

`EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v` (USDC)

## Rate Limits

The Aura API currently allows 4 requests per second and 60 requests per minute per API key and per IP address. Bursts above 10 requests per second or 150 requests per minute can trigger a 24-hour ban.

Agents should pace live tool execution. Preparing a mutation is local; confirming a mutation calls the Aura API and counts against the rate limit.

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
