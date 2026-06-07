# Aura MCP Agent Instructions

Read this before making Aura tool calls.

1. Rate limits are strict: at most 4 Aura API requests/second and 60 requests/minute per API key and per IP. For sweeps, use about one live API call every 0.5 s. Bursts above 10 requests/second or 150 requests/minute can trigger a 24-hour ban.
2. `prepare_*` tools are local and do not call Aura. `confirm_mutation`, `confirm_limit_order`, and `confirm_snipe_task` call Aura and count against rate limits.
3. Mutation tools are disabled when `read_only = true`.
4. Trading requires wallet setup. After a wallet is connected or added, the active trading wallet must have all Aura utility accounts opened and at least 1 durable nonce. If a trade, limit order, snipe, or copy-trade action fails or just do not execute, call `list_wallets` and check if anything is missing, if missing - prepare and confirm `prepare_open_util_accs`, then prepare and confirm `prepare_create_nonces` with `amount = 1`.
5. Use `list_wallets` to inspect `accounts_state.token_accounts` and `accounts_state.util_accs.durable_nonces`.
6. Batch actions when the API supports it. If follow-up limit orders are known before a trade, pass them in `MarketTrade.limit_orders` instead of calling trade and then placing orders. For snipe edits, send one `SnipeUpdate` with multiple `updates`. For copy-trade edits, send one `CtUpdate` with multiple `updates`. For wallet/mint/dev/blacklist edits, use one `ConfigPubkeys` update with multiple `pubkeys`.
7. Raw prepare tools accept `{"request": {...}}`, `{"request": "{... JSON string ...}"}`, or the raw object directly. Prefer the wrapped object form when supported by the client adapter.
8. For trading amounts, `{"Buy":{"Lamports":1000000}}` means spend 0.001 SOL. For percentages, use ratios: `"1"` means 100%, `"0.5"` means 50%. For delayed API limit orders, `activate_dur` is `[seconds, nanoseconds]`, for example `[30, 0]`.
9. `prepare_trade`, `prepare_place_limit_orders`, and `prepare_limit_order` can omit common execution knobs; Aura MCP fills slippage, tip, fee, processors, durable nonce, slot latency, price-impact, and empty trade-filter defaults.
10. Use `start_user_activity` before a workflow when events matter, remember the current `last_seq`, then call `read_user_activity` with `after_seq` when done. Call `stop_user_activity` when finished.
11. Use `explain_aura_error` before retrying unclear errors.
12. For snipe `id` arguments, first call `snipe_get_cfgs` or `list_snipe_tasks` and use a returned id. For copy-trade `id` arguments, first call `ct_get_cfgs` and use a returned id. Placeholder ids are request-shape-valid but return Aura not-found or permission errors.
13. `prepare_add_wallet.keypair_base58` is a full Solana keypair secret encoded as base58, not a wallet address/public key.
14. Never loop over all tools at full speed. Space live calls and stop after repeated transport, auth, account, nonce, balance, or rate-limit errors.

Common payloads:

```json
{"request":{"wallet":"<WALLET>","amount":{"Buy":{"Lamports":1000000}},"mint":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v","limit_orders":{"orders":[{"state":{"Api":{"id":null,"expire_dur":null,"activate_dur":[30,0]}},"order":{"target":{"Market":{"mode":"Always"}},"amount":{"SellPerc":{"amount":"1"}}},"trigger":"Immediate","wallet":"<WALLET>"}]}}}
```

```json
{"request":{"mint":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v","orders":{"orders":[{"state":{"Api":{"id":null,"expire_dur":null,"activate_dur":[60,0]}},"order":{"target":{"Market":{"mode":"Always"}},"amount":{"Buy":{"Lamports":1000000}}},"trigger":"Immediate","wallet":"<WALLET>"}]}}}
```
