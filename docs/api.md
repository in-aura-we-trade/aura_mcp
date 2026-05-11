# Aura API Methods

This MCP exposes the non-streaming Aura Rust client methods from `aura_api_client` plus an MCP-managed `user_activity` stream bridge.

All Aura API calls authenticate with the configured API key. In Rust, pass the API key as the first method argument. In MCP, run `aura-mcp login --api-key <KEY>` once and the server supplies it.

## Global Rules

Rate limits are per API key and per IP address:

- 4 requests/second
- 60 requests/minute
- Bursts above 10 requests/second or 150 requests/minute can trigger a 24-hour ban

For agents, a safe broad-test cadence is one live Aura call every 0.5 s. `prepare_*` tools are local; confirmation tools call Aura.

Trading prerequisite: after wallet connection, the active wallet must have all Aura utility accounts opened and at least 1 durable nonce. If trading, limit-order, snipe, or copy-trade execution fails with a missing account or nonce error, inspect `list_wallets`, then call and confirm `prepare_open_util_accs`, then call and confirm `prepare_create_nonces` with `amount = 1`.

Batch actions when possible:

- Use `MarketTrade.limit_orders` to attach known follow-up limit orders to the trade request. Do not call trade first and then place the same known limit orders separately.
- Use `UpdateTokenLimitOrders.orders` to place multiple limit orders for a mint in one request.
- Use one `SnipeUpdate` with multiple `updates` entries for snipe task edits.
- Use one `CtUpdate` with multiple `updates` entries for copy-trade task edits.
- Use one `ConfigPubkeys` update with multiple `pubkeys` entries for mints, devs, copy wallets, and blacklists.

## MCP Shapes

Most raw mutation tools accept the request object directly, `{ "request": { ... } }`, or
`{ "request": "<JSON string>" }`. The JSON-string form exists for tool adapters that can only
send scalar string arguments for raw payloads; prefer the object form when the client supports it.
The published MCP schema still has a top-level JSON Schema `type: "object"` for every raw tool,
because OpenAI/Codex function adapters reject schemas with a root `anyOf`.

Addresses are base58 strings in MCP JSON and `solana_address::Address` in Rust. `Lamports`, `QuoteLamports`, and `UD128` use the Rust client serde shape. In Rust examples, prefer typed constructors such as `Lamports::new`, `Usdc::new`, `QuoteLamports::Usdc`, and `udec128!`.

Percent `UD128` values are ratios, not whole percentage numbers: `0` is 0%, `UD128::ONE` / `"1"` is 100%, and `udec128!(0.5)` / `"0.5"` is 50%. Do not pass `100` for 100%.

Enums use Rust variant names in raw JSON, for example `Durable`, `Ata`, `Immediate`, `Buy`, `Sell`, `Market`, and `Always`.

`chrono::TimeDelta` fields such as `OrderState::Api.activate_dur` and `expire_dur` use tuple JSON: `[seconds, nanoseconds]`. Example for 30 seconds: `[30, 0]`.

## Friendly Defaults

For MCP-friendly `prepare_trade`, `prepare_place_limit_orders`, and `prepare_limit_order` payloads, agents may omit common execution knobs:

- `slippage`: defaults to `SLIPPAGE_DEFAULT` from `aura_api_client` (`0.2`).
- `tip`: defaults by side to `BUY_TIPS_LAMPORTS` or `SELL_TIPS_LAMPORTS`.
- `priority_fee` on `MarketTrade`, and `fee` on `RawOrder`: defaults by side to `BUY_FEE_LAMPORTS` or `SELL_FEE_LAMPORTS`.
- `procs`: defaults to `TxnProcessors::default()` from the Rust API client.
- `nonce`: defaults to `UserNonceStrategy::Durable`.
- `slot_latency`: defaults to `16` for MCP tool calls.
- `max_price_impact`: defaults to `MAX_PRICE_IMPACT_DEF` from `aura_api_client` (`0.5`) for `MarketTrade`.
- `limit_orders`: defaults to an empty `ApiOrders` list for `MarketTrade`.

Required trading intent fields are still required: `mint`, `amount`, limit-order `target`, and limit-order `wallet`.

## Core AuraRpc

`aura_user_ping` / Rust `aura().user_ping(api_key, Ping { count })`

- Purpose: health/keepalive ping.
- Request `Ping`: `count` is a caller-selected counter.
- Response `Pong`: `count` echoes the counter.

`start_user_activity`, `read_user_activity`, `user_activity_status`, `stop_user_activity` / Rust `aura().user_activity(api_key, UserActionEventSub)`

- Purpose: live stream of trade callbacks, confirmed trade events, limit-order events, token trade stats, pings, and pongs.
- MCP owns one stream and sends internal keepalive pings.
- `read_user_activity` fields: `after_seq` returns events with a higher sequence; `limit` limits returned events and is clamped to 500.

`prepare_trade` / Rust `aura().trade(api_key, MarketTrade)`

- Purpose: submit a market trade, optionally with auto-created follow-up limit orders.
- Requires utility accounts and at least 1 durable nonce on the active wallet.
- Request `MarketTrade` fields:
- `wallet`: optional wallet address. If omitted, Aura uses the active wallet.
- `amount`: `SwapAmount`, one of `Buy(QuoteLamports)`, `BuyPerc { amount }`, `SellPerc { amount }`, `SellOut(QuoteLamports)`, or `SellInit`.
- `mint`: token mint address.
- `slippage`: optional decimal slippage percent; defaults to `SLIPPAGE_DEFAULT`.
- `tip`: optional Jito/processor tip in lamports; defaults by buy/sell side.
- `procs`: optional `TxnProcessors`; defaults to `TxnProcessors::default()`.
- `nonce`: optional `UserNonceStrategy`; defaults to `Durable`.
- `priority_fee`: optional priority fee in lamports; defaults by buy/sell side.
- `slot_latency`: optional maximum slot latency; defaults to `16`.
- `expire_at`: optional UTC expiration time.
- `rpc_nonce`: optional caller nonce.
- `max_price_impact`: optional decimal price-impact bound; defaults to `MAX_PRICE_IMPACT_DEF`.
- `limit_orders`: optional `ApiOrders` to attach after trade execution; defaults to no orders.
- Response `TradeResponse`: `slot` is the slot where the trade request was accepted.

Recommendation: if a trade should create follow-up limit orders, include them in `limit_orders` on this request. That saves a separate confirmed API call and keeps the order setup tied to the trade.

`fetch_state_info` / `get_account_info` / Rust `aura().fetch_state_info(api_key, FetchInfo)`

- Purpose: active wallet, balances, optional token account state, and counters.
- Response `FetchInfoResponse`: `wallet`, `balances`, `token_accounts`, `wallets_num`, `limit_orders_num`, `ct_cfgs_num`, `snipes_num`.

`fetch_full_wallet_info` / `list_wallets` / Rust `aura().fetch_full_wallet_info(api_key, FetchFullWalletsInfoReq)`

- Purpose: active wallet, all wallets, balances, and account setup state.
- Response `FetchFullWalletsInfo`: `active`, `wallets`, `balances`, `accounts_state`.
- `accounts_state.token_accounts`: booleans for WSOL/USDC/USD1/USDT ATA and PDA accounts.
- `accounts_state.util_accs`: `pump_uva`, `pump_amm_uva`, `pump_amm_uva_ata`, `custom_nonce`, and `durable_nonces`.

`get_token_status` / Rust `aura().get_token_status(api_key, address)`

- Purpose: fetch most-liquid pool and token metadata.
- Response `TokenStatus`: `most_liq_pool` and `token_meta`.

`get_token_most_liq_pool` / Rust `aura().get_token_most_liq_pool(api_key, address)`

- Purpose: fetch the most-liquid pool for a mint.
- Response `TokenPool`: `mint`, `pool_id`, `pool_type`, `migration_status`, `price`, raw and virtual base/quote liquidity.

`get_token_meta` / Rust `aura().get_token_meta(api_key, address)`

- Purpose: fetch token metadata.
- Response `TokenMeta`: `supply`, `tax_bps`, `ticker`, `name`, `mint_auth`, `freeze_auth`, `socials`.

`get_token_trade_stats` / Rust `aura().get_token_trade_stats(api_key, address)`

- Purpose: fetch per-token balance/trade state.
- Response `TokenTradeState`: PDA/ATA balances, base kind, 2022 flag, buy/sell totals and counts, last traded slot, quote state, total quote position, mint.

`get_token_positions` / Rust `aura().get_token_positions(api_key, TokenPositionsReq)`

- Purpose: fetch token positions and SOL balance.
- Response `TokenPositions`: `v` positions and `sol_balance`.

`get_token_positions_ui` / Rust `aura().get_token_positions_ui(api_key, TokenPositionsUiReq { mint })`

- Purpose: fetch positions plus optional selected token state.
- Request `TokenPositionsUiReq`: optional `mint`.
- Response `TokenPositionsUi`: positions, SOL balance, optional selected `TokenTradeState`.

## Limit Orders

Trading prerequisite applies to limit-order execution. Open utility accounts and at least 1 durable nonce before placing executable orders.

`get_token_limit_orders` / Rust `limit_orders().get_token_limit_orders(api_key, mint)`

- Purpose: list limit orders for one token.
- Response `TokenLimitOrders`: `mint`, `orders`.

`get_limit_orders` / `list_limit_orders` / Rust `limit_orders().get_limit_orders(api_key, GetLimitOrders)`

- Purpose: list all active limit orders.
- Response `LimitOrders`: `orders`.

`prepare_place_limit_orders` / `prepare_limit_order` / Rust `limit_orders().place_limit_orders(api_key, UpdateTokenLimitOrders)`

- Purpose: add or update limit orders for a mint.
- Request `UpdateTokenLimitOrders`: `mint`, `orders`.
- Response `UpdateLimitOrdersResponse`: `total_orders`, `ids`.
- Recommendation: place all known orders for that mint in one `orders` array.

`cancel_limit_order` / `prepare_delete_limit_orders` / Rust `limit_orders().delete_limit_orders(api_key, DeleteOrders)`

- Purpose: delete selected limit orders or all orders for a mint.
- Request `DeleteOrders`: `mint`, `all`, `ids`.
- Response `DeleteLimitOrdersResponse`: `total_orders_after`.

`prepare_clear_limit_orders` / Rust `limit_orders().clear_limit_orders(api_key, ClearLimitOrders)`

- Purpose: delete all limit orders.
- Response `ClearLimitOrdersResponse`: empty response.

Limit-order request structs:

- `ApiOrders.orders`: list of `LimitOrder`.
- `LimitOrder.state`: `OrderState::Api { id, expire_dur, activate_dur }` for new API orders or `Placed { id, left_attempts, expire_timestamp_utc, status, activate_timestamp_utc }` when returned from Aura.
- `LimitOrder.order`: `RawOrder`.
- `LimitOrder.trigger`: `Immediate`, `Migration`, `DevBuy`, or `DevSell`.
- `LimitOrder.wallet`: wallet that executes the order.
- `RawOrder.slippage`: optional decimal slippage percent; defaults to `SLIPPAGE_DEFAULT`.
- `RawOrder.tip`: optional tip lamports; defaults by buy/sell side.
- `RawOrder.fee`: optional priority fee lamports; defaults by buy/sell side.
- `RawOrder.target`: `Target`.
- `RawOrder.amount`: `SwapAmount`.
- `RawOrder.procs`: optional transaction processor flags; defaults to `TxnProcessors::default()`.
- `RawOrder.nonce`: optional nonce strategy; defaults to `Durable`.
- `RawOrder.slot_latency`: optional maximum slot latency; defaults to `16`.
- `Target`: `Price`, `Profit`, `MovingPerc`, `PricePerc`, `Mcap`, or `Market { mode }`.
- `TargetMarket.mode`: `Always`, `OnlyInProfit`, or `OnlyInLoss`.

## Snipe

Trading prerequisite applies to snipe execution: the wallet selected by the task must have utility accounts and at least 1 durable nonce.

All snipe tools that take `id` require an existing snipe task id returned by `snipe_get_cfgs` or `list_snipe_tasks`. Placeholder ids are valid JSON but return not-found or permission errors from Aura.

Read tools:

- `snipe_get_cfgs`: list task ids and names.
- `snipe_get_cfg`: fetch one task.
- `snipe_get_mints`: fetch tracked mints.
- `snipe_get_devs`: fetch tracked dev wallets.
- `snipe_get_blacklist`: fetch blacklist.
- `snipe_cfg_get_limit_orders`: fetch task limit orders.
- `snipe_cfg_get_buy_txn_proc`: fetch buy processor flags.
- `snipe_cfg_get_sell_txn_proc`: fetch sell processor flags.

Mutation tools:

- `prepare_snipe_new_cfg_def`: create a default task.
- `prepare_snipe_duplicate_cfg`: duplicate task by `id`.
- `prepare_snipe_turn_off_all_tasks`: disable all snipe tasks.
- `prepare_snipe_turn_on_all_tasks`: enable all snipe tasks.
- `prepare_snipe_del_cfg`: delete task by `id`.
- `prepare_snipe_clear_all_cfgs`: delete all snipe tasks.
- `prepare_snipe_set_fields` / `update_snipe_task`: update fields with `SnipeUpdate`.

`SnipeUpdate` fields: `cfg_id` and `updates`. `SnipeUpdateField` variants include buy/sell mode and amount, config limits, left buys, buy/sell processors, limit orders, dev/mint/blacklist config pubkeys, name, mcap bounds, limit-order switches, slippage, tips, fees, slot latency, on/off flag, DEX flags, age filters, mint/freeze filters, dev buy bounds, buy/sell nonce strategy, and wallet.

Recommendation: batch all known field changes in one `SnipeUpdate.updates` array. For mints, devs, and blacklists, use one `ConfigPubkeys` value with multiple `pubkeys` instead of repeated update calls.

`SnipeTask` response fields: `cfg_id`, `user_id`, `flags`, `values`, and `triggers`.

## Copy Trade

Trading prerequisite applies to copy-trade execution: the wallet selected by the task must have utility accounts and at least 1 durable nonce.

All copy-trade tools that take `id` require an existing copy-trade task id returned by `ct_get_cfgs`. Placeholder ids are valid JSON but return not-found or permission errors from Aura.

Read tools:

- `ct_get_cfgs`: list task ids and names.
- `ct_get_cfg`: fetch one task.
- `ct_get_copy_wallets`: fetch watched wallets.
- `ct_get_buy_blacklist`: fetch buy blacklist.
- `ct_get_sell_blacklist`: fetch sell blacklist.
- `ct_cfg_get_limit_orders`: fetch task limit orders.
- `ct_cfg_get_buy_txn_proc`: fetch buy processor flags.
- `ct_cfg_get_sell_txn_proc`: fetch sell processor flags.

Mutation tools:

- `prepare_ct_new_cfg_def`: create default copy-trade task.
- `prepare_ct_duplicate_cfg`: duplicate task by `id`.
- `prepare_ct_turn_off_all_tasks`: disable all copy-trade tasks.
- `prepare_ct_turn_on_all_tasks`: enable all copy-trade tasks.
- `prepare_ct_del_cfg`: delete task by `id`.
- `prepare_ct_clear_all_cfgs`: delete all copy-trade tasks.
- `prepare_ct_set_fields`: update fields with `CtUpdate`.

`CtUpdate` fields: `cfg_id` and `updates`. `CtUpdateField` variants include config limits, left buys, buy/sell mode, fixed/percent buy/sell amounts, buy/sell processors, limit orders, watched wallets, buy/sell blacklists, blacklist switches, name, mcap bounds, slippage, tips, fees, slot latency, on/off flag, migration filters, follow modes, DEX flags, age filters, mint/freeze filters, master buy bounds, limit-order switches, reverse buy/sell, allowed buys/sells, buy/sell nonce strategy, and wallet.

Recommendation: batch all known field changes in one `CtUpdate.updates` array. For watched wallets and blacklists, use one `ConfigPubkeys` value with multiple `pubkeys` instead of repeated update calls.

`CtTask` response fields: `cfg_id`, `user_id`, `flags`, `values`, and `triggers`.

## Utilities

`prepare_change_api_key` / Rust `utils().change_api_key(api_key, ApiKeyReq)`

- Request `ApiKeyReq`: `key` is the new API key address.
- Response `ApiKeyResp`: empty.

`txn_procs_stat` / Rust `utils().txn_procs_stat(api_key, TxnProcsStatReq)`

- Purpose: inspect transaction processor performance.
- Response `TxnProcessorsStats`: per-processor `ProcessorStats` for Jito, Aura, Bloxroute, Nozomi, Next Block, Slot0, Astra, Block Razor, TPU Pen, Node1, Stellium, Helius, Soyas, Falcon, Raiden, Circular, Flashblock, Moon, and Blocksprint.
- `ProcessorStats`: total slot latency, sent/landed/error transaction counts, priority fee, and tip.

`prepare_switch_wallet` / Rust `utils().switch_wallet(api_key, address)`

- Purpose: switch active wallet.
- Request: wallet address.
- Response `Done`.

`prepare_remove_wallet` / Rust `utils().remove_wallet(api_key, RemoveWallet)`

- Request `RemoveWallet`: `to_remove`, `new` active wallet after removal.
- Response `Done`.

`prepare_add_wallet` / Rust `utils().add_wallet(api_key, Keypair)`

- Purpose: connect/add a wallet from a Solana keypair.
- MCP field: `keypair_base58`, the full Solana keypair secret encoded as base58. This is normally the 64 secret-key bytes encoded with bs58, not the wallet address/public key.
- Response: fee/lamports value returned by Aura.
- After adding a wallet, open utility accounts and create at least 1 durable nonce before trading.

`prepare_wrap_wsol` / Rust `utils().wrap_wsol(api_key, WrapWsolRequest)`

- Request `WrapWsolRequest`: `owner`, `kind` (`Ata` or `Pda`), `amount`.
- Response: transaction signature.

`prepare_unwrap_wsol` / Rust `utils().unwrap_wsol(api_key, UnwrapWsolRequest)`

- Request `UnwrapWsolRequest`: `owner`, `kind`, `amount` (`All` or `Some(u64)`).
- Response: transaction signature.

`prepare_open_ta` / Rust `utils().open_ta(api_key, OpenTaRequest)`

- Request `OpenTaRequest`: `owner`, `mint`, `kind`, `is_2022`.
- Response: transaction signature.

`prepare_open_util_accs` / Rust `utils().open_util_accs(api_key, wallet)`

- Purpose: open Aura utility accounts required for trading.
- Request: wallet address.
- Response: transaction signature.

`prepare_make_withdraw` / Rust `utils().make_withdraw(api_key, MakeWithdrawReq)`

- Request `MakeWithdrawReq`: `destination`, `amount`.
- Response `MakeWithdrawResp`: `sig`, `fee`.

`prepare_create_nonces` / Rust `utils().create_nonces(api_key, CreateNoncesReq)`

- Purpose: create durable nonce accounts. Trading needs at least one.
- Request `CreateNoncesReq`: `wallet`, `amount`.
- Response `CreateNoncesResp`: `sig`.

`prepare_update_nonces` / Rust `utils().update_nonces(api_key, UpdateNoncesReq)`

- Purpose: refresh nonce state for a wallet.
- Request `UpdateNoncesReq`: `wallet`.
- Response `UpdateNoncesResp`: `found`, `updated`.

`prepare_dex_cu_set` / Rust `utils().dex_cu_set(api_key, AtomicDexCU)`

- Purpose: set compute unit values per DEX/action.
- Request `AtomicDexCU`: `pump_buy`, `pump_sell`, `pump_amm_buy`, `pump_amm_sell`, `ray_amm_buy`, `ray_amm_sell`, `ray_cpmm_buy`, `ray_cpmm_sell`, `ray_ll_buy`, `ray_ll_sell`.
- MCP accepts `{}` to use `DexCu::init()` defaults.

`dex_cu_get` / Rust `utils().dex_cu_get(api_key, GetDexCu)`

- Purpose: read compute unit settings.
- Response `AtomicDexCU`.

## Common Struct Fields

`TxnProcessors` flags: `jito_validators`, `jito_bundled`, `aura`, `bloxroute`, `nozomi`, `next_block`, `slot0`, `astra`, `block_razor`, `node1`, `tpu_penetrator`, `helius`, `stellium`, `soyas`, `falcon`, `raiden`, `circular`, `flash_block`, `moon`, `blocksprint`, `aura_revert`.

`ConfigPubkeys`: `act` (`Insert`, `Delete`, `Clear`) and `pubkeys`.

`WalletBalances`: SOL plus WSOL/USDC/USD1/USDT ATA and PDA balances.

`WalletTaInfo`: booleans for token-account existence plus `positions`.

`UtilAccsInfo`: booleans for pump utility accounts and custom nonce plus `durable_nonces`.
