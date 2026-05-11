# Aura gRPC

The MCP binary calls Aura through the generated clients in `https://github.com/in-aura-we-trade/aura_api_client`:

- `aura`: health, account info, wallets, token data, positions, trading
- `limit_orders`: list, place, delete, clear limit orders
- `snipe`: sniper configuration reads and writes
- `utils`: wallet and utility transactions

`user_activity` is handled as a managed server-side stream inside the MCP process. The MCP server enforces one live stream per process/API key, sends `user_ping` keepalives internally, buffers recent events in memory, and exposes updates through both `read_user_activity` polling and `aura://user_activity/latest` resource notifications.

The public endpoint is configured with `api_endpoint`. The default is `http://trade.aura.rehab:40051`.

## Rate Limits

Aura applies rate limits per API key and per IP address:

- 4 requests per second
- 60 requests per minute
- More than 10 requests per second can trigger a ban
- More than 150 requests per minute can trigger a ban
- Ban duration is 24 hours

MCP agents should throttle tool calls. For broad test sweeps, keep confirmed API calls around one request every 0.5 seconds.

## Trading Account Setup

After wallet connection, the trading wallet must have all Aura utility accounts opened and at least 1 durable nonce. This applies to `Trade`, executable limit orders, sniper execution, and copy-trade execution. Use `FetchFullWalletInfo` / `list_wallets` to inspect `accounts_state`, `OpenUtilAccs` / `prepare_open_util_accs` to open utility accounts, and `CreateNonces` / `prepare_create_nonces` with `amount = 1` to create the first durable nonce.
