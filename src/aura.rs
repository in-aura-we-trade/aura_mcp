use crate::{
    config::Config,
    validation::{
        AddWalletArg, CancelLimitOrderArgs, IdArg, OptionalMintArg, RawRequestArg, parse_address,
        validate_id,
    },
};
use anyhow::{Context, Result, anyhow};
use aura_api_client::{
    client::{AuraClients, types::*},
    client_ext::UserCtx,
    consts::{
        BUY_FEE_LAMPORTS, BUY_TIPS_LAMPORTS, MAX_PRICE_IMPACT_DEF, SELL_FEE_LAMPORTS,
        SELL_TIPS_LAMPORTS, SLIPPAGE_DEFAULT,
    },
};
use chrono::{DateTime, Utc};
use decisol::Lamports;
use fastnum::UD128;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use solana_address::Address;
use solana_keypair::Keypair;
use std::{pin::Pin, str::FromStr};
use tokio_stream::Stream;
use tonic::{
    Request, Status,
    service::Interceptor,
    transport::{Channel, Endpoint},
};

#[derive(Clone, Default)]
pub struct NoopInterceptor;

impl Interceptor for NoopInterceptor {
    fn call(&mut self, request: Request<()>) -> std::result::Result<Request<()>, Status> {
        Ok(request)
    }
}

type Clients = AuraClients<NoopInterceptor, UserCtx>;
const MCP_DEFAULT_SLOT_LATENCY: u8 = 16;

#[derive(Debug, serde::Deserialize)]
struct JsonAddress(String);

impl JsonAddress {
    fn parse(self) -> Result<Address> {
        parse_address(&self.0, "address")
    }
}

#[derive(Debug, serde::Deserialize)]
struct FriendlyMarketTrade {
    wallet: Option<JsonAddress>,
    amount: SwapAmount,
    mint: JsonAddress,
    slippage: Option<UD128>,
    tip: Option<Lamports>,
    procs: Option<TxnProcessors>,
    nonce: Option<UserNonceStrategy>,
    priority_fee: Option<Lamports>,
    slot_latency: Option<u8>,
    expire_at: Option<DateTime<Utc>>,
    rpc_nonce: Option<u64>,
    max_price_impact: Option<UD128>,
    #[serde(default)]
    limit_orders: FriendlyApiOrders,
}

impl TryFrom<FriendlyMarketTrade> for MarketTrade {
    type Error = anyhow::Error;

    fn try_from(value: FriendlyMarketTrade) -> Result<Self> {
        Ok(Self {
            wallet: value.wallet.map(JsonAddress::parse).transpose()?,
            slippage: value.slippage.unwrap_or(SLIPPAGE_DEFAULT),
            tip: value.tip.unwrap_or_else(|| default_tip_for(&value.amount)),
            priority_fee: value
                .priority_fee
                .unwrap_or_else(|| default_priority_fee_for(&value.amount)),
            procs: Some(value.procs.unwrap_or_default()),
            nonce: value.nonce.unwrap_or(UserNonceStrategy::Durable),
            slot_latency: Some(value.slot_latency.unwrap_or(MCP_DEFAULT_SLOT_LATENCY)),
            amount: value.amount,
            mint: value.mint.parse()?,
            expire_at: value.expire_at,
            rpc_nonce: value.rpc_nonce,
            max_price_impact: Some(value.max_price_impact.unwrap_or(MAX_PRICE_IMPACT_DEF)),
            limit_orders: value.limit_orders.try_into()?,
        })
    }
}

#[derive(Debug, Default, serde::Deserialize)]
struct FriendlyApiOrders {
    #[serde(default)]
    orders: Vec<FriendlyApiLimitOrder>,
}

impl TryFrom<FriendlyApiOrders> for ApiOrders {
    type Error = anyhow::Error;

    fn try_from(value: FriendlyApiOrders) -> Result<Self> {
        Ok(Self {
            orders: value
                .orders
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<Vec<_>>>()?,
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct FriendlyApiLimitOrder {
    state: OrderState,
    order: FriendlyRawOrder,
    trigger: OrderEventTrigger,
    wallet: JsonAddress,
}

impl TryFrom<FriendlyApiLimitOrder> for ApiLimitOrder {
    type Error = anyhow::Error;

    fn try_from(value: FriendlyApiLimitOrder) -> Result<Self> {
        Ok(Self {
            state: value.state,
            order: value.order.into(),
            trigger: value.trigger,
            wallet: value.wallet.parse()?,
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct FriendlyRawOrder {
    slippage: Option<UD128>,
    tip: Option<Lamports>,
    fee: Option<Lamports>,
    target: Target,
    amount: SwapAmount,
    procs: Option<TxnProcessors>,
    nonce: Option<UserNonceStrategy>,
    slot_latency: Option<u8>,
}

impl From<FriendlyRawOrder> for RawOrder {
    fn from(value: FriendlyRawOrder) -> Self {
        Self {
            slippage: value.slippage.unwrap_or(SLIPPAGE_DEFAULT),
            tip: value.tip.unwrap_or_else(|| default_tip_for(&value.amount)),
            fee: value
                .fee
                .unwrap_or_else(|| default_priority_fee_for(&value.amount)),
            target: value.target,
            amount: value.amount,
            procs: value.procs.unwrap_or_default(),
            nonce: value.nonce.unwrap_or(UserNonceStrategy::Durable),
            slot_latency: value.slot_latency.unwrap_or(MCP_DEFAULT_SLOT_LATENCY),
        }
    }
}

fn default_tip_for(amount: &SwapAmount) -> Lamports {
    Lamports::new(if is_buy_amount(amount) {
        BUY_TIPS_LAMPORTS
    } else {
        SELL_TIPS_LAMPORTS
    })
}

fn default_priority_fee_for(amount: &SwapAmount) -> Lamports {
    Lamports::new(if is_buy_amount(amount) {
        BUY_FEE_LAMPORTS
    } else {
        SELL_FEE_LAMPORTS
    })
}

fn is_buy_amount(amount: &SwapAmount) -> bool {
    matches!(amount, SwapAmount::Buy(_) | SwapAmount::BuyPerc { .. })
}

#[derive(Debug, serde::Deserialize)]
struct FriendlyUpdateTokenLimitOrders {
    mint: JsonAddress,
    orders: FriendlyApiOrders,
}

impl TryFrom<FriendlyUpdateTokenLimitOrders> for UpdateTokenLimitOrders {
    type Error = anyhow::Error;

    fn try_from(value: FriendlyUpdateTokenLimitOrders) -> Result<Self> {
        Ok(Self {
            mint: value.mint.parse()?,
            orders: value.orders.try_into()?,
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct FriendlyDeleteOrders {
    mint: JsonAddress,
    all: bool,
    ids: Vec<OrderId>,
}

impl TryFrom<FriendlyDeleteOrders> for DeleteOrders {
    type Error = anyhow::Error;

    fn try_from(value: FriendlyDeleteOrders) -> Result<Self> {
        Ok(Self {
            mint: value.mint.parse()?,
            all: value.all,
            ids: value.ids,
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct FriendlyApiKeyReq {
    key: JsonAddress,
}

impl TryFrom<FriendlyApiKeyReq> for ApiKeyReq {
    type Error = anyhow::Error;

    fn try_from(value: FriendlyApiKeyReq) -> Result<Self> {
        Ok(Self {
            key: value.key.parse()?,
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct FriendlyRemoveWallet {
    to_remove: JsonAddress,
    new: JsonAddress,
}

impl TryFrom<FriendlyRemoveWallet> for RemoveWallet {
    type Error = anyhow::Error;

    fn try_from(value: FriendlyRemoveWallet) -> Result<Self> {
        Ok(Self {
            to_remove: value.to_remove.parse()?,
            new: value.new.parse()?,
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct FriendlyWrapWsolRequest {
    owner: JsonAddress,
    kind: TokenAccountKindUi,
    amount: u64,
}

impl TryFrom<FriendlyWrapWsolRequest> for WrapWsolRequest {
    type Error = anyhow::Error;

    fn try_from(value: FriendlyWrapWsolRequest) -> Result<Self> {
        Ok(Self {
            owner: value.owner.parse()?,
            kind: value.kind,
            amount: value.amount,
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct FriendlyUnwrapWsolRequest {
    owner: JsonAddress,
    kind: TokenAccountKindUi,
    amount: UnwrapWsolAmount,
}

impl TryFrom<FriendlyUnwrapWsolRequest> for UnwrapWsolRequest {
    type Error = anyhow::Error;

    fn try_from(value: FriendlyUnwrapWsolRequest) -> Result<Self> {
        Ok(Self {
            owner: value.owner.parse()?,
            kind: value.kind,
            amount: value.amount,
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct FriendlyOpenTaRequest {
    owner: JsonAddress,
    mint: JsonAddress,
    kind: TokenAccountKindUi,
    is_2022: bool,
}

impl TryFrom<FriendlyOpenTaRequest> for OpenTaRequest {
    type Error = anyhow::Error;

    fn try_from(value: FriendlyOpenTaRequest) -> Result<Self> {
        Ok(Self {
            owner: value.owner.parse()?,
            mint: value.mint.parse()?,
            kind: value.kind,
            is_2022: value.is_2022,
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct FriendlyMakeWithdrawReq {
    destination: JsonAddress,
    amount: Lamports,
}

impl TryFrom<FriendlyMakeWithdrawReq> for MakeWithdrawReq {
    type Error = anyhow::Error;

    fn try_from(value: FriendlyMakeWithdrawReq) -> Result<Self> {
        Ok(Self {
            destination: value.destination.parse()?,
            amount: value.amount,
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct FriendlyCreateNoncesReq {
    wallet: JsonAddress,
    amount: u8,
}

impl TryFrom<FriendlyCreateNoncesReq> for CreateNoncesReq {
    type Error = anyhow::Error;

    fn try_from(value: FriendlyCreateNoncesReq) -> Result<Self> {
        Ok(Self {
            wallet: value.wallet.parse()?,
            amount: value.amount,
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct FriendlyUpdateNoncesReq {
    wallet: JsonAddress,
}

impl TryFrom<FriendlyUpdateNoncesReq> for UpdateNoncesReq {
    type Error = anyhow::Error;

    fn try_from(value: FriendlyUpdateNoncesReq) -> Result<Self> {
        Ok(Self {
            wallet: value.wallet.parse()?,
        })
    }
}

#[derive(Clone)]
pub struct AuraApi {
    clients: Clients,
    api_key: Address,
}

#[derive(Debug, Serialize)]
pub struct ToolEnvelope<T: Serialize> {
    pub ok: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
}

impl AuraApi {
    pub async fn connect(config: &Config) -> Result<Self> {
        config.validate_for_api()?;
        let api_key = Address::from_str(config.api_key.as_deref().unwrap_or_default())
            .context("api_key is not a valid Aura API key")?;
        let endpoint = Endpoint::from_shared(config.api_endpoint.clone())
            .context("api_endpoint is not a valid gRPC endpoint URL")?;
        let channel = endpoint
            .connect()
            .await
            .with_context(|| format!("failed to connect to {}", config.api_endpoint))?;

        Ok(Self {
            clients: AuraClients::new(channel, NoopInterceptor),
            api_key,
        })
    }

    pub fn from_channel(channel: Channel, api_key: Address) -> Self {
        Self {
            clients: AuraClients::new(channel, NoopInterceptor),
            api_key,
        }
    }

    pub async fn open_user_activity(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<UserAction, Status>> + Send>>> {
        let mut client = self.clients.aura();
        let stream = client
            .user_activity(self.api_key, UserActionEventSub)
            .await?
            .into_inner();
        Ok(Box::pin(stream))
    }

    pub async fn user_ping_internal(&self) -> Result<()> {
        self.clients
            .aura()
            .user_ping(self.api_key, Ping { count: 1 })
            .await?;
        Ok(())
    }

    pub async fn get_aura_status(&self) -> Result<Value> {
        match self
            .clients
            .aura()
            .user_ping(self.api_key, Ping { count: 1 })
            .await
        {
            Ok(pong) => success("Aura API responded to ping", pong.into_inner()),
            Err(ping_error) => {
                let info = self
                    .clients
                    .aura()
                    .fetch_state_info(self.api_key, FetchInfo)
                    .await?
                    .into_inner();
                success(
                    "Aura API responded to fetch_state_info; user_ping failed",
                    json!({
                        "ping_error": ping_error.to_string(),
                        "wallet": info.wallet.to_string(),
                        "wallets_num": info.wallets_num,
                        "limit_orders_num": info.limit_orders_num,
                        "ct_cfgs_num": info.ct_cfgs_num,
                        "snipes_num": info.snipes_num,
                    }),
                )
            }
        }
    }

    pub async fn get_account_info(&self) -> Result<Value> {
        let info = self
            .clients
            .aura()
            .fetch_state_info(self.api_key, FetchInfo)
            .await?
            .into_inner();
        success("Fetched account info", info)
    }

    pub async fn list_wallets(&self) -> Result<Value> {
        let info = self
            .clients
            .aura()
            .fetch_full_wallet_info(self.api_key, FetchFullWalletsInfoReq)
            .await?
            .into_inner();
        success(
            "Fetched wallets",
            json!({
                "active": info.active.to_string(),
                "wallets": info.wallets.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "balances": info.balances,
                "accounts_state": info.accounts_state,
            }),
        )
    }

    pub async fn list_snipe_tasks(&self) -> Result<Value> {
        let cfgs = self
            .clients
            .snipe()
            .snipe_get_cfgs(self.api_key, GetCfgIds)
            .await?
            .into_inner();
        success("Fetched snipe tasks", cfgs)
    }

    pub async fn list_limit_orders(&self) -> Result<Value> {
        let orders = self
            .clients
            .limit_orders()
            .get_limit_orders(self.api_key, GetLimitOrders)
            .await?
            .into_inner();
        success("Fetched limit orders", orders)
    }

    pub async fn get_bot_status(&self) -> Result<Value> {
        let info = self
            .clients
            .aura()
            .fetch_state_info(self.api_key, FetchInfo)
            .await?
            .into_inner();
        success(
            "Fetched bot status",
            json!({
                "active_wallet": info.wallet.to_string(),
                "wallets_num": info.wallets_num,
                "limit_orders_num": info.limit_orders_num,
                "ct_cfgs_num": info.ct_cfgs_num,
                "snipes_num": info.snipes_num,
            }),
        )
    }

    pub async fn cancel_limit_order(&self, args: &CancelLimitOrderArgs) -> Result<Value> {
        let mint = parse_address(&args.mint, "mint")?;
        let ids = args.order_ids.iter().copied().map(OrderId).collect();
        let resp = self
            .clients
            .limit_orders()
            .delete_limit_orders(
                self.api_key,
                DeleteOrders {
                    mint,
                    all: args.all,
                    ids,
                },
            )
            .await?
            .into_inner();
        success("Cancelled limit order request submitted", resp)
    }

    pub async fn call_read(&self, name: &str, args: Value) -> Result<Value> {
        match name {
            "aura_user_ping" => {
                let count = args.get("count").and_then(Value::as_u64).unwrap_or(1);
                match self
                    .clients
                    .aura()
                    .user_ping(self.api_key, Ping { count })
                    .await
                {
                    Ok(pong) => success("Aura API responded to ping", pong.into_inner()),
                    Err(error) => Ok(error_value(
                        format!("aura_user_ping failed: {error}"),
                        "Use get_aura_status for a health check with fallback, or retry later.",
                    )),
                }
            }
            "fetch_state_info" | "get_account_info" => self.get_account_info().await,
            "fetch_full_wallet_info" | "list_wallets" => self.list_wallets().await,
            "get_token_status" => {
                let address = address_arg(args, "address")?;
                success(
                    "Fetched token status",
                    self.clients
                        .aura()
                        .get_token_status(self.api_key, address)
                        .await?
                        .into_inner(),
                )
            }
            "get_token_most_liq_pool" => {
                let address = address_arg(args, "address")?;
                success(
                    "Fetched token most-liquid pool",
                    self.clients
                        .aura()
                        .get_token_most_liq_pool(self.api_key, address)
                        .await?
                        .into_inner(),
                )
            }
            "get_token_meta" => {
                let address = address_arg(args, "address")?;
                success(
                    "Fetched token metadata",
                    self.clients
                        .aura()
                        .get_token_meta(self.api_key, address)
                        .await?
                        .into_inner(),
                )
            }
            "get_token_trade_stats" => {
                let address = address_arg(args, "address")?;
                success(
                    "Fetched token trade stats",
                    self.clients
                        .aura()
                        .get_token_trade_stats(self.api_key, address)
                        .await?
                        .into_inner(),
                )
            }
            "get_token_positions" => success(
                "Fetched token positions",
                self.clients
                    .aura()
                    .get_token_positions(self.api_key, TokenPositionsReq)
                    .await?
                    .into_inner(),
            ),
            "get_token_positions_ui" => {
                let args: OptionalMintArg = decode_args(args)?;
                let mint = args
                    .mint
                    .as_deref()
                    .map(|mint| parse_address(mint, "mint"))
                    .transpose()?;
                success_debug(
                    "Fetched token positions UI",
                    &self
                        .clients
                        .aura()
                        .get_token_positions_ui(self.api_key, TokenPositionsUiReq { mint })
                        .await?
                        .into_inner(),
                )
            }
            "get_token_limit_orders" => {
                let address = address_arg(args, "address")?;
                success(
                    "Fetched token limit orders",
                    self.clients
                        .limit_orders()
                        .get_token_limit_orders(self.api_key, address)
                        .await?
                        .into_inner(),
                )
            }
            "get_limit_orders" | "list_limit_orders" => self.list_limit_orders().await,
            "snipe_get_cfgs" | "list_snipe_tasks" => self.list_snipe_tasks().await,
            "snipe_get_cfg" => {
                let id = snipe_id_arg(args)?;
                success(
                    "Fetched snipe task",
                    self.clients
                        .snipe()
                        .snipe_get_cfg(self.api_key, id)
                        .await?
                        .into_inner(),
                )
            }
            "snipe_get_mints" => {
                let id = snipe_id_arg(args)?;
                success(
                    "Fetched snipe task mints",
                    self.clients
                        .snipe()
                        .snipe_get_mints(self.api_key, id)
                        .await?
                        .into_inner(),
                )
            }
            "snipe_get_devs" => {
                let id = snipe_id_arg(args)?;
                success(
                    "Fetched snipe task devs",
                    self.clients
                        .snipe()
                        .snipe_get_devs(self.api_key, id)
                        .await?
                        .into_inner(),
                )
            }
            "snipe_get_blacklist" => {
                let id = snipe_id_arg(args)?;
                success(
                    "Fetched snipe task blacklist",
                    self.clients
                        .snipe()
                        .snipe_get_blacklist(self.api_key, id)
                        .await?
                        .into_inner(),
                )
            }
            "snipe_cfg_get_limit_orders" => {
                let id = snipe_id_arg(args)?;
                success(
                    "Fetched snipe task limit orders",
                    self.clients
                        .snipe()
                        .snipe_cfg_get_limit_orders(self.api_key, id)
                        .await?
                        .into_inner(),
                )
            }
            "snipe_cfg_get_buy_txn_proc" => {
                let id = snipe_id_arg(args)?;
                success(
                    "Fetched snipe task buy transaction processors",
                    self.clients
                        .snipe()
                        .snipe_cfg_get_buy_txn_proc(self.api_key, id)
                        .await?
                        .into_inner(),
                )
            }
            "snipe_cfg_get_sell_txn_proc" => {
                let id = snipe_id_arg(args)?;
                success(
                    "Fetched snipe task sell transaction processors",
                    self.clients
                        .snipe()
                        .snipe_cfg_get_sell_txn_proc(self.api_key, id)
                        .await?
                        .into_inner(),
                )
            }
            "ct_get_cfgs" => success(
                "Fetched copy-trade tasks",
                self.clients
                    .ct()
                    .ct_get_cfgs(self.api_key, GetCfgIds)
                    .await?
                    .into_inner(),
            ),
            "ct_get_cfg" => {
                let id = ct_id_arg(args)?;
                success(
                    "Fetched copy-trade task",
                    self.clients
                        .ct()
                        .ct_get_cfg(self.api_key, id)
                        .await?
                        .into_inner(),
                )
            }
            "ct_get_copy_wallets" => {
                let id = ct_id_arg(args)?;
                success(
                    "Fetched copy-trade wallets",
                    self.clients
                        .ct()
                        .ct_get_copy_wallets(self.api_key, id)
                        .await?
                        .into_inner(),
                )
            }
            "ct_get_buy_blacklist" => {
                let id = ct_id_arg(args)?;
                success(
                    "Fetched copy-trade buy blacklist",
                    self.clients
                        .ct()
                        .ct_get_buy_blacklist(self.api_key, id)
                        .await?
                        .into_inner(),
                )
            }
            "ct_get_sell_blacklist" => {
                let id = ct_id_arg(args)?;
                success(
                    "Fetched copy-trade sell blacklist",
                    self.clients
                        .ct()
                        .ct_get_sell_blacklist(self.api_key, id)
                        .await?
                        .into_inner(),
                )
            }
            "ct_cfg_get_limit_orders" => {
                let id = ct_id_arg(args)?;
                success(
                    "Fetched copy-trade limit orders",
                    self.clients
                        .ct()
                        .ct_cfg_get_limit_orders(self.api_key, id)
                        .await?
                        .into_inner(),
                )
            }
            "ct_cfg_get_buy_txn_proc" => {
                let id = ct_id_arg(args)?;
                success(
                    "Fetched copy-trade buy transaction processors",
                    self.clients
                        .ct()
                        .ct_cfg_get_buy_txn_proc(self.api_key, id)
                        .await?
                        .into_inner(),
                )
            }
            "ct_cfg_get_sell_txn_proc" => {
                let id = ct_id_arg(args)?;
                success(
                    "Fetched copy-trade sell transaction processors",
                    self.clients
                        .ct()
                        .ct_cfg_get_sell_txn_proc(self.api_key, id)
                        .await?
                        .into_inner(),
                )
            }
            "txn_procs_stat" => success(
                "Fetched transaction processor stats",
                self.clients
                    .utils()
                    .txn_procs_stat(self.api_key, TxnProcsStatReq)
                    .await?
                    .into_inner(),
            ),
            "dex_cu_get" => success(
                "Fetched DEX compute unit settings",
                self.clients
                    .utils()
                    .dex_cu_get(self.api_key, GetDexCu)
                    .await?
                    .into_inner(),
            ),
            "user_activity" => success(
                "Aura user_activity is a streaming API managed by MCP activity tools",
                json!({
                    "streaming": true,
                    "one_shot_tool": false,
                    "start_tool": "start_user_activity",
                    "read_tool": "read_user_activity",
                    "status_tool": "user_activity_status",
                    "stop_tool": "stop_user_activity",
                    "resource": "aura://user_activity/latest"
                }),
            ),
            _ => Err(anyhow!("unknown Aura read API method {name}")),
        }
    }

    pub async fn call_mutation(&self, name: &str, args: Value) -> Result<Value> {
        match name {
            "trade" => success(
                "Submitted trade",
                self.clients
                    .aura()
                    .trade(
                        self.api_key,
                        decode_friendly_request::<FriendlyMarketTrade, MarketTrade>(args)?,
                    )
                    .await?
                    .into_inner(),
            ),
            "place_limit_orders" | "prepare_limit_order" => success(
                "Placed limit orders",
                self.clients
                    .limit_orders()
                    .place_limit_orders(
                        self.api_key,
                        decode_friendly_request::<
                            FriendlyUpdateTokenLimitOrders,
                            UpdateTokenLimitOrders,
                        >(args)?,
                    )
                    .await?
                    .into_inner(),
            ),
            "delete_limit_orders" => success(
                "Deleted limit orders",
                self.clients
                    .limit_orders()
                    .delete_limit_orders(
                        self.api_key,
                        decode_friendly_request::<FriendlyDeleteOrders, DeleteOrders>(args)?,
                    )
                    .await?
                    .into_inner(),
            ),
            "clear_limit_orders" => success(
                "Cleared limit orders",
                self.clients
                    .limit_orders()
                    .clear_limit_orders(self.api_key, ClearLimitOrders)
                    .await?
                    .into_inner(),
            ),
            "snipe_new_cfg_def" | "prepare_snipe_task" => success(
                "Created default snipe task",
                self.clients
                    .snipe()
                    .snipe_new_cfg_def(self.api_key, CreateDefCfg)
                    .await?
                    .into_inner(),
            ),
            "snipe_duplicate_cfg" => {
                let id = snipe_id_arg(args)?;
                success(
                    "Duplicated snipe task",
                    self.clients
                        .snipe()
                        .snipe_duplicate_cfg(self.api_key, id)
                        .await?
                        .into_inner(),
                )
            }
            "snipe_turn_off_all_tasks" => success(
                "Turned off all snipe tasks",
                self.clients
                    .snipe()
                    .snipe_turn_off_all_tasks(self.api_key, TurnOffAll)
                    .await?
                    .into_inner(),
            ),
            "snipe_turn_on_all_tasks" => success(
                "Turned on all snipe tasks",
                self.clients
                    .snipe()
                    .snipe_turn_on_all_tasks(self.api_key, TurnOnAll)
                    .await?
                    .into_inner(),
            ),
            "snipe_del_cfg" => {
                let id = snipe_id_arg(args)?;
                success(
                    "Deleted snipe task",
                    self.clients
                        .snipe()
                        .snipe_del_cfg(self.api_key, id)
                        .await?
                        .into_inner(),
                )
            }
            "snipe_clear_all_cfgs" => success(
                "Cleared all snipe tasks",
                self.clients
                    .snipe()
                    .snipe_clear_all_cfgs(self.api_key, ClearAll)
                    .await?
                    .into_inner(),
            ),
            "snipe_set_fields" | "update_snipe_task" => success(
                "Updated snipe task",
                self.clients
                    .snipe()
                    .snipe_set_fields(self.api_key, decode_request::<SnipeUpdate>(args)?)
                    .await?
                    .into_inner(),
            ),
            "ct_new_cfg_def" => success(
                "Created default copy-trade task",
                self.clients
                    .ct()
                    .ct_new_cfg_def(self.api_key, CreateDefCfg)
                    .await?
                    .into_inner(),
            ),
            "ct_duplicate_cfg" => {
                let id = ct_id_arg(args)?;
                success(
                    "Duplicated copy-trade task",
                    self.clients
                        .ct()
                        .ct_duplicate_cfg(self.api_key, id)
                        .await?
                        .into_inner(),
                )
            }
            "ct_turn_off_all_tasks" => success(
                "Turned off all copy-trade tasks",
                self.clients
                    .ct()
                    .ct_turn_off_all_tasks(self.api_key, TurnOffAll)
                    .await?
                    .into_inner(),
            ),
            "ct_turn_on_all_tasks" => success(
                "Turned on all copy-trade tasks",
                self.clients
                    .ct()
                    .ct_turn_on_all_tasks(self.api_key, TurnOnAll)
                    .await?
                    .into_inner(),
            ),
            "ct_del_cfg" => {
                let id = ct_id_arg(args)?;
                success(
                    "Deleted copy-trade task",
                    self.clients
                        .ct()
                        .ct_del_cfg(self.api_key, id)
                        .await?
                        .into_inner(),
                )
            }
            "ct_clear_all_cfgs" => success(
                "Cleared all copy-trade tasks",
                self.clients
                    .ct()
                    .ct_clear_all_cfgs(self.api_key, ClearAll)
                    .await?
                    .into_inner(),
            ),
            "ct_set_fields" => success(
                "Updated copy-trade task",
                self.clients
                    .ct()
                    .ct_set_fields(self.api_key, decode_request::<CtUpdate>(args)?)
                    .await?
                    .into_inner(),
            ),
            "change_api_key" => success(
                "Changed API key",
                self.clients
                    .utils()
                    .change_api_key(
                        self.api_key,
                        decode_friendly_request::<FriendlyApiKeyReq, ApiKeyReq>(args)?,
                    )
                    .await?
                    .into_inner(),
            ),
            "switch_wallet" => {
                let address = address_arg(args, "address")?;
                success(
                    "Switched active wallet",
                    self.clients
                        .utils()
                        .switch_wallet(self.api_key, address)
                        .await?
                        .into_inner(),
                )
            }
            "remove_wallet" => success(
                "Removed wallet",
                self.clients
                    .utils()
                    .remove_wallet(
                        self.api_key,
                        decode_friendly_request::<FriendlyRemoveWallet, RemoveWallet>(args)?,
                    )
                    .await?
                    .into_inner(),
            ),
            "add_wallet" => {
                let args: AddWalletArg = decode_args(args)?;
                let keypair = Keypair::try_from_base58_string(&args.keypair_base58)
                    .context("keypair_base58 is not a valid Solana keypair")?;
                success(
                    "Added wallet",
                    self.clients
                        .utils()
                        .add_wallet(self.api_key, keypair)
                        .await?
                        .into_inner(),
                )
            }
            "wrap_wsol" => success(
                "Wrapped SOL",
                self.clients
                    .utils()
                    .wrap_wsol(
                        self.api_key,
                        decode_friendly_request::<FriendlyWrapWsolRequest, WrapWsolRequest>(args)?,
                    )
                    .await?
                    .into_inner(),
            ),
            "unwrap_wsol" => success(
                "Unwrapped WSOL",
                self.clients
                    .utils()
                    .unwrap_wsol(
                        self.api_key,
                        decode_friendly_request::<FriendlyUnwrapWsolRequest, UnwrapWsolRequest>(
                            args,
                        )?,
                    )
                    .await?
                    .into_inner(),
            ),
            "open_ta" => success(
                "Opened token account",
                self.clients
                    .utils()
                    .open_ta(
                        self.api_key,
                        decode_friendly_request::<FriendlyOpenTaRequest, OpenTaRequest>(args)?,
                    )
                    .await?
                    .into_inner(),
            ),
            "open_util_accs" => {
                let address = address_arg(args, "address")?;
                success(
                    "Opened utility accounts",
                    self.clients
                        .utils()
                        .open_util_accs(self.api_key, address)
                        .await?
                        .into_inner(),
                )
            }
            "make_withdraw" => success(
                "Submitted withdraw",
                self.clients
                    .utils()
                    .make_withdraw(
                        self.api_key,
                        decode_friendly_request::<FriendlyMakeWithdrawReq, MakeWithdrawReq>(args)?,
                    )
                    .await?
                    .into_inner(),
            ),
            "create_nonces" => success(
                "Created nonces",
                self.clients
                    .utils()
                    .create_nonces(
                        self.api_key,
                        decode_friendly_request::<FriendlyCreateNoncesReq, CreateNoncesReq>(args)?,
                    )
                    .await?
                    .into_inner(),
            ),
            "update_nonces" => success(
                "Updated nonces",
                self.clients
                    .utils()
                    .update_nonces(
                        self.api_key,
                        decode_friendly_request::<FriendlyUpdateNoncesReq, UpdateNoncesReq>(args)?,
                    )
                    .await?
                    .into_inner(),
            ),
            "dex_cu_set" => success(
                "Updated DEX compute unit settings",
                self.clients
                    .utils()
                    .dex_cu_set(self.api_key, decode_dex_cu(args)?)
                    .await?
                    .into_inner(),
            ),
            _ => Err(anyhow!("unknown Aura mutation API method {name}")),
        }
    }
}

pub fn validate_mutation_request(name: &str, args: Value) -> Result<()> {
    match name {
        "trade" => {
            decode_friendly_request::<FriendlyMarketTrade, MarketTrade>(args)?;
        }
        "place_limit_orders" | "prepare_limit_order" => {
            decode_friendly_request::<FriendlyUpdateTokenLimitOrders, UpdateTokenLimitOrders>(
                args,
            )?;
        }
        "delete_limit_orders" => {
            decode_friendly_request::<FriendlyDeleteOrders, DeleteOrders>(args)?;
        }
        "clear_limit_orders" => {}
        "snipe_new_cfg_def" | "prepare_snipe_task" => {}
        "snipe_duplicate_cfg" | "snipe_del_cfg" => {
            snipe_id_arg(args)?;
        }
        "snipe_turn_off_all_tasks" | "snipe_turn_on_all_tasks" | "snipe_clear_all_cfgs" => {}
        "snipe_set_fields" | "update_snipe_task" => {
            decode_request::<SnipeUpdate>(args)?;
        }
        "ct_new_cfg_def" => {}
        "ct_duplicate_cfg" | "ct_del_cfg" => {
            ct_id_arg(args)?;
        }
        "ct_turn_off_all_tasks" | "ct_turn_on_all_tasks" | "ct_clear_all_cfgs" => {}
        "ct_set_fields" => {
            decode_request::<CtUpdate>(args)?;
        }
        "change_api_key" => {
            decode_friendly_request::<FriendlyApiKeyReq, ApiKeyReq>(args)?;
        }
        "switch_wallet" => {
            address_arg(args, "address")?;
        }
        "remove_wallet" => {
            decode_friendly_request::<FriendlyRemoveWallet, RemoveWallet>(args)?;
        }
        "add_wallet" => {
            let args: AddWalletArg = decode_args(args)?;
            Keypair::try_from_base58_string(&args.keypair_base58)
                .context("keypair_base58 is not a valid Solana keypair")?;
        }
        "wrap_wsol" => {
            decode_friendly_request::<FriendlyWrapWsolRequest, WrapWsolRequest>(args)?;
        }
        "unwrap_wsol" => {
            decode_friendly_request::<FriendlyUnwrapWsolRequest, UnwrapWsolRequest>(args)?;
        }
        "open_ta" => {
            decode_friendly_request::<FriendlyOpenTaRequest, OpenTaRequest>(args)?;
        }
        "open_util_accs" => {
            address_arg(args, "address")?;
        }
        "make_withdraw" => {
            decode_friendly_request::<FriendlyMakeWithdrawReq, MakeWithdrawReq>(args)?;
        }
        "create_nonces" => {
            decode_friendly_request::<FriendlyCreateNoncesReq, CreateNoncesReq>(args)?;
        }
        "update_nonces" => {
            decode_friendly_request::<FriendlyUpdateNoncesReq, UpdateNoncesReq>(args)?;
        }
        "dex_cu_set" => {
            decode_dex_cu(args)?;
        }
        _ => return Err(anyhow!("unknown Aura mutation API method {name}")),
    }
    Ok(())
}

fn decode_args<T: DeserializeOwned>(args: Value) -> Result<T> {
    serde_json::from_value(args).context("invalid tool arguments")
}

fn decode_friendly_request<F, T>(args: Value) -> Result<T>
where
    F: DeserializeOwned + TryInto<T>,
    F::Error: Into<anyhow::Error>,
{
    serde_json::from_value::<F>(raw_request_value(args))
        .context("invalid request payload")
        .and_then(|request| request.try_into().map_err(Into::into))
}

fn decode_request<T: DeserializeOwned>(args: Value) -> Result<T> {
    serde_json::from_value(raw_request_value(args)).context("invalid request payload")
}

fn decode_dex_cu(args: Value) -> Result<DexCu> {
    let request = raw_request_value(args);
    if request.as_object().is_some_and(serde_json::Map::is_empty) {
        return Ok(DexCu::init());
    }
    serde_json::from_value(request).context("invalid request payload")
}

fn raw_request_value(args: Value) -> Value {
    let request = if let Ok(wrapped) = serde_json::from_value::<RawRequestArg>(args.clone()) {
        wrapped.request
    } else {
        args
    };

    if let Value::String(request) = request {
        serde_json::from_str(&request).unwrap_or(Value::String(request))
    } else {
        request
    }
}

fn address_arg(args: Value, default_field: &str) -> Result<Address> {
    if let Ok(arg) = serde_json::from_value::<crate::validation::AddressArg>(args.clone()) {
        return parse_address(&arg.address, default_field);
    }
    for field in ["address", "mint", "wallet", "owner", default_field] {
        if let Some(value) = args.get(field).and_then(Value::as_str) {
            return parse_address(value, field);
        }
    }
    Err(anyhow!("missing address argument"))
}

fn snipe_id_arg(args: Value) -> Result<SnipeTaskId> {
    let id = id_arg(args, "id")?;
    Ok(SnipeTaskId(id))
}

fn ct_id_arg(args: Value) -> Result<CtTaskId> {
    let id = id_arg(args, "id")?;
    Ok(CtTaskId(id))
}

fn id_arg(args: Value, field: &str) -> Result<i64> {
    let arg: IdArg = decode_args(args)?;
    validate_id(arg.id, field)?;
    Ok(arg.id)
}

fn success_debug<T: std::fmt::Debug>(message: impl Into<String>, data: &T) -> Result<Value> {
    Ok(json!({
        "ok": true,
        "message": message.into(),
        "data": format!("{data:?}")
    }))
}

pub fn explain_aura_error(error: &str) -> Value {
    let lower = error.to_ascii_lowercase();
    let (message, hint) = if lower.contains("auth") || lower.contains("unauth") {
        (
            "Aura rejected authentication",
            "Run `aura-mcp login --api-key <KEY>` and confirm the key came from the Aura Telegram bot API tab.",
        )
    } else if lower.contains("connect") || lower.contains("transport") || lower.contains("timeout")
    {
        (
            "Aura endpoint could not be reached",
            "Check api_endpoint in ~/.config/aura/mcp.toml and verify the network can reach the gRPC endpoint.",
        )
    } else if lower.contains("slippage") {
        (
            "The request appears to violate slippage constraints",
            "Use a bounded slippage_bps value and inspect token liquidity before retrying.",
        )
    } else if lower.contains("balance") || lower.contains("insufficient") {
        (
            "The active wallet may not have enough balance",
            "Use get_account_info or list_wallets, then adjust the amount or active wallet.",
        )
    } else {
        (
            "Aura returned an error that is not recognized locally",
            "Inspect the original error string and retry with the smallest read-only query that reproduces it.",
        )
    };

    json!({
        "ok": true,
        "message": message,
        "data": {
            "input": error,
            "hint": hint
        }
    })
}

pub fn error_value(error: impl ToString, hint: impl ToString) -> Value {
    json!({
        "ok": false,
        "error": error.to_string(),
        "hint": hint.to_string()
    })
}

fn success<T: Serialize>(message: impl Into<String>, data: T) -> Result<Value> {
    serde_json::to_value(ToolEnvelope {
        ok: true,
        message: message.into(),
        data: Some(data),
    })
    .map_err(|err| anyhow!("failed to serialize Aura response: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use decisol::QuoteLamports;

    #[test]
    fn empty_dex_cu_uses_client_defaults() {
        let decoded = decode_dex_cu(json!({})).unwrap();
        let defaults = DexCu::init();
        assert_eq!(decoded.pump_buy, defaults.pump_buy);
        assert_eq!(decoded.pump_sell, defaults.pump_sell);
        assert_eq!(decoded.pump_amm_buy, defaults.pump_amm_buy);
        assert_eq!(decoded.pump_amm_sell, defaults.pump_amm_sell);
        assert_eq!(decoded.ray_amm_buy, defaults.ray_amm_buy);
        assert_eq!(decoded.ray_amm_sell, defaults.ray_amm_sell);
        assert_eq!(decoded.ray_cpmm_buy, defaults.ray_cpmm_buy);
        assert_eq!(decoded.ray_cpmm_sell, defaults.ray_cpmm_sell);
        assert_eq!(decoded.ray_ll_buy, defaults.ray_ll_buy);
        assert_eq!(decoded.ray_ll_sell, defaults.ray_ll_sell);
    }

    #[test]
    fn friendly_market_trade_uses_execution_defaults() {
        let mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
        let decoded = MarketTrade::try_from(FriendlyMarketTrade {
            wallet: None,
            amount: SwapAmount::Buy(QuoteLamports::Lamports(Lamports::new(10_000_u64))),
            mint: JsonAddress(mint.into()),
            slippage: None,
            tip: None,
            procs: None,
            nonce: None,
            priority_fee: None,
            slot_latency: None,
            expire_at: None,
            rpc_nonce: None,
            max_price_impact: None,
            limit_orders: FriendlyApiOrders::default(),
        })
        .unwrap();

        assert_eq!(decoded.slippage, SLIPPAGE_DEFAULT);
        assert_eq!(decoded.tip, Lamports::new(BUY_TIPS_LAMPORTS));
        assert_eq!(decoded.priority_fee, Lamports::new(BUY_FEE_LAMPORTS));
        assert_eq!(decoded.slot_latency, Some(MCP_DEFAULT_SLOT_LATENCY));
        assert_eq!(decoded.max_price_impact, Some(MAX_PRICE_IMPACT_DEF));
        assert!(decoded.procs.is_some());
        assert!(matches!(decoded.nonce, UserNonceStrategy::Durable));
        assert!(decoded.limit_orders.orders.is_empty());
    }

    #[test]
    fn friendly_market_trade_accepts_json_string_request_wrapper() {
        let wallet = "AURAXd1nDoqtUDnjTFeedapcbSTid5XYhYpm2hhN6wd9";
        let mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
        let request = json!({
            "wallet": wallet,
            "amount": {"Buy": {"Lamports": 1_000_000}},
            "mint": mint,
            "limit_orders": {
                "orders": [{
                    "state": {
                        "Api": {
                            "id": null,
                            "expire_dur": null,
                            "activate_dur": [30, 0]
                        }
                    },
                    "order": {
                        "target": {"Market": {"mode": "Always"}},
                        "amount": {"SellPerc": {"amount": "1"}}
                    },
                    "trigger": "Immediate",
                    "wallet": wallet
                }]
            }
        });
        let payload = json!({ "request": request.to_string() });

        let decoded = decode_friendly_request::<FriendlyMarketTrade, MarketTrade>(payload).unwrap();

        assert_eq!(decoded.mint, parse_address(mint, "mint").unwrap());
        assert_eq!(decoded.limit_orders.orders.len(), 1);
        assert!(matches!(
            decoded.limit_orders.orders[0].order.amount,
            SwapAmount::SellPerc { .. }
        ));
    }

    #[test]
    fn friendly_raw_order_uses_sell_defaults() {
        let order = RawOrder::from(FriendlyRawOrder {
            slippage: None,
            tip: None,
            fee: None,
            target: Target::Market {
                mode: MarketExecuteMode::Always,
            },
            amount: SwapAmount::SellPerc { amount: UD128::ONE },
            procs: None,
            nonce: None,
            slot_latency: None,
        });

        assert_eq!(order.slippage, SLIPPAGE_DEFAULT);
        assert_eq!(order.tip, Lamports::new(SELL_TIPS_LAMPORTS));
        assert_eq!(order.fee, Lamports::new(SELL_FEE_LAMPORTS));
        assert_eq!(order.slot_latency, MCP_DEFAULT_SLOT_LATENCY);
        assert!(matches!(order.nonce, UserNonceStrategy::Durable));
    }

    #[test]
    fn friendly_limit_order_payload_accepts_execution_defaults() {
        let wallet = "AURAXd1nDoqtUDnjTFeedapcbSTid5XYhYpm2hhN6wd9";
        let mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
        let payload = json!({
            "mint": mint,
            "orders": {
                "orders": [{
                    "state": {
                        "Api": {
                            "id": null,
                            "expire_dur": null,
                            "activate_dur": [30, 0]
                        }
                    },
                    "order": {
                        "target": {"Market": {"mode": "Always"}},
                        "amount": {"SellPerc": {"amount": "1"}}
                    },
                    "trigger": "Immediate",
                    "wallet": wallet
                }]
            }
        });

        let decoded = decode_friendly_request::<
            FriendlyUpdateTokenLimitOrders,
            UpdateTokenLimitOrders,
        >(payload)
        .unwrap();
        let order = &decoded.orders.orders[0].order;
        assert_eq!(order.slippage, SLIPPAGE_DEFAULT);
        assert_eq!(order.slot_latency, MCP_DEFAULT_SLOT_LATENCY);
        assert!(matches!(order.nonce, UserNonceStrategy::Durable));
    }
}
