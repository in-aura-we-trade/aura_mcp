# Rust Example

```rust
use aura_api_client::{
    client::{AuraClients, types::{FetchInfo, Ping}},
    client_ext::UserCtx,
};
use tonic::transport::Channel;

// The MCP server uses the generated Aura clients directly and supplies the
// configured API key as the per-call UserCtx payload.
```

For normal AI-agent usage, run:

```bash
aura-mcp login --api-key <KEY>
aura-mcp serve
```
