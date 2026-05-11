# Aura MCP Overview

Aura MCP is a local stdio Model Context Protocol server for AI agents. It runs on the user machine and connects to Aura's gRPC API with the Rust client types from `aura_api_client`.

Default API endpoint: `http://trade.aura.rehab:40051`

The server exposes compact read-only tools for account, wallet, limit order, snipe task, and health checks. State-changing tools use a prepare/confirm flow and respect `read_only = true`.

Agents should read `aura://instructions/agent` before live calls. The key constraints are: throttle Aura calls to stay below 4 requests/second and 60 requests/minute per API key/IP, and make sure any trading wallet has all Aura utility accounts opened plus at least 1 durable nonce.

Agents should batch actions when the API supports it. A trade can include follow-up limit orders, and snipe/copy-trade task edits can include multiple field updates in one call.

Raw prepare tools can be called directly from tool discovery: use `{"request": {...}}` when object-valued arguments are supported, or `{"request": "{... JSON string ...}"}` when an adapter only exposes scalar raw payloads. The server decodes both forms before calling Aura.

Use `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v` as a known USDC mint example for token and pool lookups.
