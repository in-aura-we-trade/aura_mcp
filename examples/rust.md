# Rust Client Examples

The Rust client methods take the Aura API key as the first argument. The MCP stores that key in local config and supplies it for tool calls, but direct Rust code must pass it explicitly.

Rate limit direct Rust calls the same way as MCP calls: at most 4 requests/second and 60 requests/minute per API key and IP. For broad sweeps, use about one live call every 0.5 seconds. Bursts above 10 requests/second or 150 requests/minute can trigger a 24-hour ban.

After wallet connection, the wallet must have all Aura utility accounts opened and at least 1 durable nonce before trading. This applies to market trades, executable limit orders, snipe execution, and copy-trade execution.

## Connect

```rust
use aura_api_client::{
    client::{
        AuraClients,
        types::{
            ApiLimitOrder, ApiOrders, CreateNoncesReq, FetchFullWalletsInfoReq, FetchInfo,
            MarketExecuteMode, MarketTrade, OrderEventTrigger, OrderState, RawOrder, Target,
            TokenAccountKindUi, TxnProcessors, UpdateTokenLimitOrders, UserNonceStrategy,
        },
    },
    client_ext::UserCtx,
    consts::AURA_API_LINK,
};
use chrono::TimeDelta;
use decisol::{Lamports, QuoteLamports, Usdc, udec128};
use fastnum::UD128;
use solana_address::Address;
use solana_keypair::Keypair;
use std::str::FromStr;
use tonic::{
    Request, Status,
    service::Interceptor,
    transport::Endpoint,
};

#[derive(Clone, Default)]
struct NoopInterceptor;

impl Interceptor for NoopInterceptor {
    fn call(&mut self, request: Request<()>) -> Result<Request<()>, Status> {
        Ok(request)
    }
}

type Clients = AuraClients<NoopInterceptor, UserCtx>;

async fn connect(api_key: &str) -> anyhow::Result<(Clients, Address)> {
    let api_key = Address::from_str(api_key)?;
    let channel = Endpoint::from_shared(AURA_API_LINK.to_owned())?
        .connect()
        .await?;
    Ok((AuraClients::new(channel, NoopInterceptor), api_key))
}
```

If you are using only the MCP binary, configure it instead:

```bash
aura-mcp login --api-key <KEY> --read-only false
aura-mcp serve
```

## Wallet Connection And Setup

Add or switch the wallet first. After that, explicitly open utility accounts and create at least 1 durable nonce.

```rust
# use aura_api_client::client::{AuraClients, types::{CreateNoncesReq, FetchFullWalletsInfoReq}};
# use aura_api_client::client_ext::UserCtx;
# use solana_address::Address;
# use solana_keypair::Keypair;
# use std::str::FromStr;
# type Clients<I> = AuraClients<I, UserCtx>;
# async fn example<I>(clients: Clients<I>, api_key: Address, wallet_keypair_base58: &str, wallet: Address) -> anyhow::Result<()>
# where I: tonic::service::Interceptor + Clone {
let keypair = Keypair::try_from_base58_string(wallet_keypair_base58)?;

// Connect/add a wallet. The API key is passed as the first argument.
clients.utils().add_wallet(api_key, keypair).await?;

// Optional if you already have multiple wallets and need this one active.
clients.utils().switch_wallet(api_key, wallet).await?;

// Inspect setup state. Check accounts_state.util_accs.durable_nonces.
let wallets = clients
    .aura()
    .fetch_full_wallet_info(api_key, FetchFullWalletsInfoReq)
    .await?
    .into_inner();

// Open all Aura utility accounts required by trading.
if !wallets.accounts_state.util_accs.is_util_accs_created() {
    clients.utils().open_util_accs(api_key, wallet).await?;
}

// Create at least one durable nonce account before trading.
if wallets.accounts_state.util_accs.durable_nonces == 0 {
    clients
        .utils()
        .create_nonces(api_key, CreateNoncesReq { wallet, amount: 1 })
        .await?;
}
# Ok(())
# }
```

Equivalent MCP flow:

```json
{"name": "prepare_add_wallet", "arguments": {"keypair_base58": "<BASE58_KEYPAIR>"}}
```

```json
{"name": "prepare_open_util_accs", "arguments": {"address": "<WALLET>"}}
```

```json
{"name": "prepare_create_nonces", "arguments": {"request": {"wallet": "<WALLET>", "amount": 1}}}
```

Call `confirm_mutation` with each returned `confirmation_id`.

## Default Processor Flags

```rust
use aura_api_client::client::types::TxnProcessors;

let processors = TxnProcessors::default();
```

## Market Trade With Auto Limit Orders

This submits a market buy on `mint` and attaches one delayed market limit order. The limit order uses `Target::Market { mode: Always }`, so it executes at market when its activation delay is reached.

Parameter notes:

- `wallet`: optional wallet override. Use `None` to use the active wallet.
- `amount`: swap size. `SwapAmount::Buy` means quote input; here, USDC quote lamports.
- `mint`: token mint to trade.
- `slippage`: decimal slippage percent.
- `tip`: processor/Jito tip in lamports.
- `procs`: transaction processor selection.
- `nonce`: use `Durable` only after at least 1 durable nonce exists.
- `priority_fee`: priority fee in lamports.
- `slot_latency`: optional maximum accepted slot latency.
- `expire_at`: optional UTC expiration.
- `rpc_nonce`: optional caller nonce.
- `max_price_impact`: optional decimal price-impact guard.
- `filters`: optional trade filters such as min/max market cap.
- `limit_orders`: optional auto limit orders attached to the trade.

Percent values are ratios, not whole percentage numbers: `UD128::ZERO` is 0%, `UD128::ONE` is 100%, and `udec128!(0.5)` is 50%.

```rust
use aura_api_client::client::types::{
    ApiLimitOrder, ApiOrders, MarketExecuteMode, MarketTrade, OrderEventTrigger, OrderState,
    RawOrder, SwapAmount, Target, TradeFilters, TxnProcessors, UserNonceStrategy,
};
use chrono::TimeDelta;
use decisol::{Lamports, QuoteLamports, Usdc, udec128};
use fastnum::UD128;

# async fn example<I>(
#     clients: aura_api_client::client::AuraClients<I, aura_api_client::client_ext::UserCtx>,
#     api_key: solana_address::Address,
#     wallet: solana_address::Address,
#     mint: solana_address::Address,
# ) -> anyhow::Result<()>
# where I: tonic::service::Interceptor + Clone {
let processors = TxnProcessors::default();

let auto_limit = ApiLimitOrder {
    state: OrderState::Api {
        id: None,
        expire_dur: None,
        activate_dur: Some(TimeDelta::seconds(30)),
    },
    order: RawOrder {
        slippage: udec128!(0.5),
        tip: Lamports::new(1_000_000),      // 0.001 SOL
        fee: Lamports::new(100_000),        // 0.0001 SOL priority fee
        target: Target::Market {
            mode: MarketExecuteMode::Always,
        },
        amount: SwapAmount::SellPerc {
            amount: UD128::ONE,
        },
        procs: processors.clone(),
        nonce: UserNonceStrategy::Durable,
        slot_latency: 2,
    },
    trigger: OrderEventTrigger::Immediate,
    wallet,
};

let trade = MarketTrade {
    wallet: Some(wallet),
    amount: SwapAmount::Buy(QuoteLamports::Usdc(Usdc::new(10_000))), // 0.01 USDC
    mint,
    slippage: udec128!(0.5),
    tip: Lamports::new(1_000_000),
    procs: Some(processors),
    nonce: UserNonceStrategy::Durable,
    priority_fee: Lamports::new(100_000),
    slot_latency: Some(2),
    expire_at: None,
    rpc_nonce: None,
    max_price_impact: Some(udec128!(1.0)),
    filters: TradeFilters::default(),
    limit_orders: ApiOrders {
        orders: vec![auto_limit],
    },
};

let response = clients.aura().trade(api_key, trade).await?.into_inner();
println!("trade accepted at slot {}", response.slot);
# Ok(())
# }
```

## DCA-Style Delayed Market Limit Order

This places a market buy limit order for USDC, activated 30 seconds later. It uses 0.01 SOL size, a 0.001 SOL tip, and a 0.0001 SOL priority fee. For a sell version, replace `SwapAmount::Buy(...)` with `SwapAmount::SellPerc { amount: UD128::ONE }` or another sell amount.

```rust
use aura_api_client::client::types::{
    ApiLimitOrder, ApiOrders, MarketExecuteMode, OrderEventTrigger, OrderState, RawOrder,
    SwapAmount, Target, TxnProcessors, UpdateTokenLimitOrders, UserNonceStrategy,
};
use chrono::TimeDelta;
use decisol::{Lamports, QuoteLamports, udec128};
use fastnum::UD128;

# async fn example<I>(
#     clients: aura_api_client::client::AuraClients<I, aura_api_client::client_ext::UserCtx>,
#     api_key: solana_address::Address,
#     wallet: solana_address::Address,
# ) -> anyhow::Result<()>
# where I: tonic::service::Interceptor + Clone {
let usdc = solana_address::Address::from_str(
    "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
)?;

let dca_order = ApiLimitOrder {
    state: OrderState::Api {
        id: None,
        expire_dur: None,
        activate_dur: Some(TimeDelta::seconds(30)),
    },
    order: RawOrder {
        slippage: udec128!(0.5),
        tip: Lamports::new(1_000_000),       // 0.001 SOL
        fee: Lamports::new(100_000),         // 0.0001 SOL priority fee
        target: Target::Market {
            mode: MarketExecuteMode::Always,
        },
        amount: SwapAmount::Buy(QuoteLamports::Lamports(Lamports::new(10_000_000))), // 0.01 SOL
        procs: TxnProcessors::default(),
        nonce: UserNonceStrategy::Durable,
        slot_latency: 2,
    },
    trigger: OrderEventTrigger::Immediate,
    wallet,
};

let response = clients
    .limit_orders()
    .place_limit_orders(
        api_key,
        UpdateTokenLimitOrders {
            mint: usdc,
            orders: ApiOrders {
                orders: vec![dca_order],
            },
        },
    )
    .await?
    .into_inner();

println!("total orders: {}, new ids: {:?}", response.total_orders, response.ids);
# Ok(())
# }
```
