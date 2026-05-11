# Aura MCP Overview

Aura MCP is a local stdio Model Context Protocol server for AI agents. It runs on the user machine and connects to Aura's gRPC API with the Rust client types from `aura_api_client`.

Default API endpoint: `http://trade.aura.rehab:40051`

The server exposes compact read-only tools for account, wallet, limit order, snipe task, and health checks. State-changing tools use a prepare/confirm flow and respect `read_only = true`.

Use `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v` as a known USDC mint example for token and pool lookups.
