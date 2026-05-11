use crate::{
    activity::{ACTIVITY_URI, ActivityManager, ReadActivityArgs},
    aura::{AuraApi, error_value, explain_aura_error, validate_mutation_request},
    config::{Config, config_path},
    validation::{
        AddWalletArg, AddressArg, CancelLimitOrderArgs, ConfirmationArgs, ExplainAuraErrorArgs,
        IdArg, OptionalMintArg, validate_cancel_limit_order,
    },
};
use anyhow::{Context, Result, anyhow};
use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{self, BufRead, Write},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use uuid::Uuid;

const CONFIRMATION_TTL: Duration = Duration::from_secs(300);
const RATE_LIMIT_NOTICE: &str = "Rate limit: Aura allows 4 API requests/second and 60 requests/minute per API key and per IP. Throttle live Aura calls to about one request every 0.5 seconds during sweeps. Bursts above 10 requests/second or 150 requests/minute can trigger a 24-hour ban.";
const TRADING_PREREQ_NOTICE: &str = "Trading prerequisite: after wallet connection, the active wallet must have all Aura utility accounts opened and at least 1 durable nonce. If trading, limit-order, snipe, or copy-trade execution fails with a missing account or nonce error, inspect list_wallets, then prepare_open_util_accs and prepare_create_nonces with amount=1.";
const BATCHING_NOTICE: &str = "Batch when the API supports it: MarketTrade.limit_orders can create follow-up limit orders in the same trade call; SnipeUpdate.updates and CtUpdate.updates can change multiple fields in one set_fields call; ConfigPubkeys can insert, delete, or clear multiple pubkeys in one update.";
const RAW_REQUEST_NOTICE: &str = "Raw request tools accept arguments as {\"request\": <object>} or {\"request\": \"<JSON string>\"}. They also tolerate the raw request object directly. Addresses are base58 strings; TimeDelta uses [seconds, nanoseconds]; UD128 percents are ratios, so \"1\" is 100%.";

#[derive(Debug, Clone)]
enum PendingAction {
    Mutation { method: String, args: Value },
    CancelLimitOrder(CancelLimitOrderArgs),
}

#[derive(Default)]
pub struct ConfirmationStore {
    actions: HashMap<String, (Instant, PendingAction)>,
}

impl ConfirmationStore {
    fn insert(&mut self, action: PendingAction) -> String {
        self.gc();
        let id = Uuid::new_v4().to_string();
        self.actions.insert(id.clone(), (Instant::now(), action));
        id
    }

    fn take(&mut self, id: &str) -> Option<PendingAction> {
        self.gc();
        self.actions.remove(id).map(|(_, action)| action)
    }

    fn gc(&mut self) {
        self.actions
            .retain(|_, (created, _)| created.elapsed() <= CONFIRMATION_TTL);
    }
}

pub fn read_only_blocks_mutation(config: &Config) -> bool {
    config.read_only
}

pub async fn serve() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aura_mcp=info,warn".into()),
        )
        .try_init()
        .ok();

    let path = config_path()?;
    let config = Config::load(&path).unwrap_or_else(|err| {
        tracing::warn!(error = %err, "using default config until a tool needs Aura API access");
        Config::default()
    });
    let mut server = McpServer::new(config);
    server.run().await
}

struct McpServer {
    config: Config,
    confirmations: ConfirmationStore,
    api: Option<AuraApi>,
    activity: ActivityManager,
}

impl McpServer {
    fn new(config: Config) -> Self {
        let (notifications, _rx) = mpsc::unbounded_channel();
        Self::new_with_output(config, notifications)
    }

    fn new_with_output(config: Config, notifications: mpsc::UnboundedSender<Value>) -> Self {
        Self {
            activity: ActivityManager::new(config.clone(), notifications),
            config,
            confirmations: ConfirmationStore::default(),
            api: None,
        }
    }

    async fn run(&mut self) -> Result<()> {
        let stdin = io::stdin();
        let (output_tx, mut output_rx) = mpsc::unbounded_channel::<Value>();
        self.activity = ActivityManager::new(self.config.clone(), output_tx.clone());

        let writer = std::thread::spawn(move || -> Result<()> {
            let mut stdout = io::stdout().lock();
            while let Some(message) = output_rx.blocking_recv() {
                write_message(&mut stdout, &message)?;
            }
            Ok(())
        });

        for line in stdin.lock().lines() {
            let line = line.context("failed to read MCP stdin")?;
            if line.trim().is_empty() {
                continue;
            }
            let request = match serde_json::from_str::<RpcRequest>(&line) {
                Ok(request) => request,
                Err(err) => {
                    let _ = output_tx.send(rpc_error(None, -32700, format!("parse error: {err}")));
                    continue;
                }
            };

            if request.id.is_none() {
                continue;
            }

            let id = request.id.clone();
            let response = match self.handle(request).await {
                Ok(result) => rpc_result(id, result),
                Err(err) => rpc_error(id, -32603, err.to_string()),
            };
            if output_tx.send(response).is_err() {
                break;
            }
        }
        drop(output_tx);
        writer
            .join()
            .map_err(|_| anyhow!("stdout writer panicked"))??;
        Ok(())
    }

    async fn api(&mut self) -> Result<&AuraApi> {
        if self.api.is_none() {
            self.api = Some(AuraApi::connect(&self.config).await?);
        }
        Ok(self.api.as_ref().unwrap())
    }

    async fn handle(&mut self, request: RpcRequest) -> Result<Value> {
        match request.method.as_str() {
            "initialize" => Ok(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {},
                    "resources": {
                        "subscribe": true
                    }
                },
                "serverInfo": {
                    "name": "aura-mcp",
                    "version": env!("CARGO_PKG_VERSION")
                }
            })),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "resources/list" => Ok(json!({ "resources": resources() })),
            "resources/read" => self.read_resource(request.params).await,
            "resources/subscribe" => self.subscribe_resource(request.params).await,
            "resources/unsubscribe" => self.unsubscribe_resource(request.params).await,
            "tools/call" => match self.call_tool(request.params).await {
                Ok(result) => Ok(result),
                Err(err) => Ok(tool_result(
                    error_value(
                        err,
                        "Check tool arguments, Aura config, or run `aura-mcp doctor`.",
                    ),
                    true,
                )),
            },
            "prompts/list" => Ok(json!({ "prompts": [] })),
            _ => Err(anyhow!("unknown MCP method {}", request.method)),
        }
    }

    async fn read_resource(&self, params: Option<Value>) -> Result<Value> {
        let params: ReadResourceParams = serde_json::from_value(params.unwrap_or_default())
            .context("invalid resource params")?;
        if params.uri == ACTIVITY_URI {
            return Ok(json!({
                "contents": [{
                    "uri": params.uri,
                    "mimeType": "application/json",
                    "text": serde_json::to_string_pretty(&self.activity.snapshot().await)?
                }]
            }));
        }
        let text = resource_text(&params.uri).ok_or_else(|| anyhow!("unknown resource URI"))?;
        Ok(json!({
            "contents": [{
                "uri": params.uri,
                "mimeType": "text/markdown",
                "text": text
            }]
        }))
    }

    async fn subscribe_resource(&self, params: Option<Value>) -> Result<Value> {
        let params: ReadResourceParams = serde_json::from_value(params.unwrap_or_default())
            .context("invalid resource params")?;
        self.activity.subscribe(&params.uri).await?;
        Ok(json!({}))
    }

    async fn unsubscribe_resource(&self, params: Option<Value>) -> Result<Value> {
        let params: ReadResourceParams = serde_json::from_value(params.unwrap_or_default())
            .context("invalid resource params")?;
        self.activity.unsubscribe(&params.uri).await?;
        Ok(json!({}))
    }

    async fn call_tool(&mut self, params: Option<Value>) -> Result<Value> {
        let params: CallToolParams =
            serde_json::from_value(params.unwrap_or_default()).context("invalid tool params")?;
        let args = params.arguments.unwrap_or_else(|| json!({}));

        let value = match params.name.as_str() {
            "get_aura_status" => self.api().await?.get_aura_status().await?,
            "get_account_info" => self.api().await?.get_account_info().await?,
            "list_wallets" => self.api().await?.list_wallets().await?,
            "list_snipe_tasks" => self.api().await?.list_snipe_tasks().await?,
            "list_limit_orders" => self.api().await?.list_limit_orders().await?,
            "get_bot_status" => self.api().await?.get_bot_status().await?,
            "start_user_activity" => serde_json::to_value(self.activity.start().await)?,
            "read_user_activity" => {
                let args: ReadActivityArgs =
                    serde_json::from_value(args).context("invalid read_user_activity args")?;
                serde_json::to_value(self.activity.read(args).await)?
            }
            "user_activity_status" => serde_json::to_value(self.activity.status().await)?,
            "stop_user_activity" => self.activity.stop().await,
            "explain_aura_error" => {
                let args: ExplainAuraErrorArgs =
                    serde_json::from_value(args).context("invalid explain_aura_error args")?;
                explain_aura_error(&args.error)
            }
            "confirm_limit_order" => {
                self.confirm_named_mutation(args, "place_limit_orders")
                    .await?
            }
            "cancel_limit_order" => self.prepare_cancel_limit_order(args)?,
            "confirm_mutation" => self.confirm_mutation(args).await?,
            "confirm_snipe_task" => {
                self.confirm_named_mutation(args, "snipe_new_cfg_def")
                    .await?
            }
            name if is_prepare_tool(name) => self.prepare_api_mutation(name, args)?,
            name if is_read_tool(name) => self.api().await?.call_read(name, args).await?,
            _ => error_value(
                "unknown tool",
                "Call list-tools to inspect supported Aura MCP tools.",
            ),
        };

        Ok(tool_result(value, false))
    }

    fn mutation_blocked(&self) -> Option<Value> {
        read_only_blocks_mutation(&self.config).then(|| {
            error_value(
                "read-only mode is enabled",
                "Set read_only = false in the Aura MCP config to enable state-changing tools.",
            )
        })
    }

    fn prepare_api_mutation(&mut self, tool_name: &str, args: Value) -> Result<Value> {
        if let Some(blocked) = self.mutation_blocked() {
            return Ok(blocked);
        }
        let method = mutation_method_for_prepare_tool(tool_name)
            .ok_or_else(|| anyhow!("unknown mutation tool {tool_name}"))?
            .to_owned();
        validate_mutation_request(&method, args.clone()).context("invalid mutation request")?;
        let id = self.confirmations.insert(PendingAction::Mutation {
            method: method.clone(),
            args,
        });
        Ok(json!({
            "ok": true,
            "message": "Mutation prepared. Call confirm_mutation with the confirmation_id to execute it.",
            "data": {
                "confirmation_id": id,
                "method": method,
                "next_tool": "confirm_mutation"
            }
        }))
    }

    fn prepare_cancel_limit_order(&mut self, args: Value) -> Result<Value> {
        if let Some(blocked) = self.mutation_blocked() {
            return Ok(blocked);
        }
        let args: CancelLimitOrderArgs =
            serde_json::from_value(args).context("invalid cancel_limit_order args")?;
        validate_cancel_limit_order(&args)?;
        let summary = if args.all {
            format!("Cancel all limit orders for {}", args.mint)
        } else {
            format!("Cancel limit orders {:?} for {}", args.order_ids, args.mint)
        };
        let id = self
            .confirmations
            .insert(PendingAction::CancelLimitOrder(args));
        Ok(json!({
            "ok": true,
            "message": "Cancel request prepared. Call confirm_mutation with the confirmation_id to execute it.",
            "data": {
                "confirmation_id": id,
                "summary": summary,
                "next_tool": "confirm_mutation"
            }
        }))
    }

    async fn confirm_mutation(&mut self, args: Value) -> Result<Value> {
        if let Some(blocked) = self.mutation_blocked() {
            return Ok(blocked);
        }
        let args: ConfirmationArgs =
            serde_json::from_value(args).context("invalid confirm_mutation args")?;
        match self.confirmations.take(&args.confirmation_id) {
            Some(PendingAction::CancelLimitOrder(cancel)) => {
                self.api().await?.cancel_limit_order(&cancel).await
            }
            Some(PendingAction::Mutation { method, args }) => {
                self.api().await?.call_mutation(&method, args).await
            }
            None => Ok(error_value(
                "unknown or expired confirmation_id",
                "Prepare the action again; confirmations expire after five minutes.",
            )),
        }
    }

    async fn confirm_named_mutation(&mut self, args: Value, expected: &str) -> Result<Value> {
        if let Some(blocked) = self.mutation_blocked() {
            return Ok(blocked);
        }
        let args: ConfirmationArgs =
            serde_json::from_value(args).context("invalid confirmation args")?;
        match self.confirmations.take(&args.confirmation_id) {
            Some(PendingAction::Mutation { method, args }) if method == expected => {
                self.api().await?.call_mutation(&method, args).await
            }
            Some(other) => {
                self.confirmations.insert(other);
                Ok(error_value(
                    "confirmation_id is for a different action",
                    "Use confirm_mutation or the matching confirm tool for this prepared mutation.",
                ))
            }
            None => Ok(error_value(
                "unknown or expired confirmation_id",
                "Prepare the action again; confirmations expire after five minutes.",
            )),
        }
    }
}

#[derive(Debug, Deserialize)]
struct RpcRequest {
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct CallToolParams {
    name: String,
    #[serde(default)]
    arguments: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ReadResourceParams {
    uri: String,
}

fn rpc_result(id: Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Option<Value>, code: i64, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn write_message(stdout: &mut impl Write, message: &Value) -> Result<()> {
    serde_json::to_writer(&mut *stdout, message)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

fn tool_result(value: Value, is_error: bool) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
        }],
        "isError": is_error
    })
}

fn schema<T: JsonSchema>() -> Value {
    serde_json::to_value(schema_for!(T)).unwrap_or_else(|_| json!({"type": "object"}))
}

fn empty_schema() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

fn is_read_tool(name: &str) -> bool {
    READ_TOOLS.iter().any(|(tool, _, _)| *tool == name)
}

fn is_prepare_tool(name: &str) -> bool {
    mutation_method_for_prepare_tool(name).is_some()
}

fn mutation_method_for_prepare_tool(name: &str) -> Option<&'static str> {
    match name {
        "prepare_limit_order" | "prepare_place_limit_orders" => Some("place_limit_orders"),
        "prepare_snipe_task" | "prepare_snipe_new_cfg_def" => Some("snipe_new_cfg_def"),
        "update_snipe_task" => Some("snipe_set_fields"),
        name => MUTATION_TOOLS
            .iter()
            .find_map(|(tool, method, _, _)| (*tool == name).then_some(*method)),
    }
}

fn tool_schema(kind: ToolSchema) -> Value {
    match kind {
        ToolSchema::Empty => empty_schema(),
        ToolSchema::Address => schema::<AddressArg>(),
        ToolSchema::Id => schema::<IdArg>(),
        ToolSchema::OptionalMint => schema::<OptionalMintArg>(),
        ToolSchema::Raw => raw_request_schema(),
        ToolSchema::AddWallet => schema::<AddWalletArg>(),
        ToolSchema::CancelLimitOrder => schema::<CancelLimitOrderArgs>(),
        ToolSchema::Confirmation => schema::<ConfirmationArgs>(),
        ToolSchema::Explain => schema::<ExplainAuraErrorArgs>(),
    }
}

fn raw_request_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "RawRequestArg",
        "description": RAW_REQUEST_NOTICE,
        "anyOf": [
            {
                "type": "object",
                "additionalProperties": false,
                "required": ["request"],
                "properties": {
                    "request": {
                        "description": "Aura client request object, or the same request encoded as a JSON string for adapters that only support scalar raw payloads.",
                        "anyOf": [
                            { "type": "object", "additionalProperties": true },
                            { "type": "array" },
                            { "type": "string" },
                            { "type": "number" },
                            { "type": "integer" },
                            { "type": "boolean" },
                            { "type": "null" }
                        ]
                    }
                }
            },
            {
                "type": "object",
                "description": "Direct raw Aura request object. Prefer the wrapped request form when the client supports it.",
                "additionalProperties": true
            }
        ],
        "examples": [
            {
                "request": {
                    "wallet": "AURAXd1nDoqtUDnjTFeedapcbSTid5XYhYpm2hhN6wd9",
                    "amount": { "Buy": { "Lamports": 1000000 } },
                    "mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
                }
            },
            {
                "request": "{\"wallet\":\"AURAXd1nDoqtUDnjTFeedapcbSTid5XYhYpm2hhN6wd9\",\"amount\":{\"Buy\":{\"Lamports\":1000000}},\"mint\":\"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v\"}"
            }
        ]
    })
}

#[derive(Clone, Copy)]
enum ToolSchema {
    Empty,
    Address,
    Id,
    OptionalMint,
    Raw,
    AddWallet,
    CancelLimitOrder,
    Confirmation,
    Explain,
}

const READ_TOOLS: &[(&str, &str, ToolSchema)] = &[
    (
        "aura_user_ping",
        "Ping Aura with an optional count.",
        ToolSchema::Empty,
    ),
    (
        "fetch_state_info",
        "Fetch active wallet, balances, and counters.",
        ToolSchema::Empty,
    ),
    (
        "fetch_full_wallet_info",
        "Fetch all wallets, balances, and utility account state.",
        ToolSchema::Empty,
    ),
    (
        "get_token_status",
        "Fetch token status by address.",
        ToolSchema::Address,
    ),
    (
        "get_token_most_liq_pool",
        "Fetch the most liquid pool for a token.",
        ToolSchema::Address,
    ),
    (
        "get_token_meta",
        "Fetch token metadata.",
        ToolSchema::Address,
    ),
    (
        "get_token_trade_stats",
        "Fetch token trade stats.",
        ToolSchema::Address,
    ),
    (
        "get_token_positions",
        "Fetch token positions.",
        ToolSchema::Empty,
    ),
    (
        "get_token_positions_ui",
        "Fetch token positions UI data, optionally selected by mint.",
        ToolSchema::OptionalMint,
    ),
    (
        "get_token_limit_orders",
        "Fetch limit orders for one token.",
        ToolSchema::Address,
    ),
    (
        "get_limit_orders",
        "Fetch all limit orders.",
        ToolSchema::Empty,
    ),
    ("snipe_get_cfgs", "Fetch snipe task IDs.", ToolSchema::Empty),
    ("snipe_get_cfg", "Fetch one snipe task.", ToolSchema::Id),
    (
        "snipe_get_mints",
        "Fetch tracked mints for a snipe task.",
        ToolSchema::Id,
    ),
    (
        "snipe_get_devs",
        "Fetch tracked devs for a snipe task.",
        ToolSchema::Id,
    ),
    (
        "snipe_get_blacklist",
        "Fetch blacklist for a snipe task.",
        ToolSchema::Id,
    ),
    (
        "snipe_cfg_get_limit_orders",
        "Fetch snipe task limit orders.",
        ToolSchema::Id,
    ),
    (
        "snipe_cfg_get_buy_txn_proc",
        "Fetch snipe task buy transaction processors.",
        ToolSchema::Id,
    ),
    (
        "snipe_cfg_get_sell_txn_proc",
        "Fetch snipe task sell transaction processors.",
        ToolSchema::Id,
    ),
    (
        "ct_get_cfgs",
        "Fetch copy-trade task IDs.",
        ToolSchema::Empty,
    ),
    ("ct_get_cfg", "Fetch one copy-trade task.", ToolSchema::Id),
    (
        "ct_get_copy_wallets",
        "Fetch copy-trade watched wallets.",
        ToolSchema::Id,
    ),
    (
        "ct_get_buy_blacklist",
        "Fetch copy-trade buy blacklist.",
        ToolSchema::Id,
    ),
    (
        "ct_get_sell_blacklist",
        "Fetch copy-trade sell blacklist.",
        ToolSchema::Id,
    ),
    (
        "ct_cfg_get_limit_orders",
        "Fetch copy-trade limit orders.",
        ToolSchema::Id,
    ),
    (
        "ct_cfg_get_buy_txn_proc",
        "Fetch copy-trade buy transaction processors.",
        ToolSchema::Id,
    ),
    (
        "ct_cfg_get_sell_txn_proc",
        "Fetch copy-trade sell transaction processors.",
        ToolSchema::Id,
    ),
    (
        "txn_procs_stat",
        "Fetch transaction processor stats.",
        ToolSchema::Empty,
    ),
    (
        "dex_cu_get",
        "Fetch DEX compute-unit settings.",
        ToolSchema::Empty,
    ),
    (
        "user_activity",
        "Report that Aura's streaming user activity API is not a one-shot MCP tool.",
        ToolSchema::Empty,
    ),
];

const MUTATION_TOOLS: &[(&str, &str, &str, ToolSchema)] = &[
    (
        "prepare_trade",
        "trade",
        "Prepare an Aura market trade from a raw MarketTrade request.",
        ToolSchema::Raw,
    ),
    (
        "prepare_place_limit_orders",
        "place_limit_orders",
        "Prepare raw Aura limit-order placement.",
        ToolSchema::Raw,
    ),
    (
        "prepare_delete_limit_orders",
        "delete_limit_orders",
        "Prepare raw Aura limit-order deletion.",
        ToolSchema::Raw,
    ),
    (
        "prepare_clear_limit_orders",
        "clear_limit_orders",
        "Prepare clearing all Aura limit orders.",
        ToolSchema::Empty,
    ),
    (
        "prepare_snipe_new_cfg_def",
        "snipe_new_cfg_def",
        "Prepare creating a default snipe task.",
        ToolSchema::Empty,
    ),
    (
        "prepare_snipe_duplicate_cfg",
        "snipe_duplicate_cfg",
        "Prepare duplicating a snipe task.",
        ToolSchema::Id,
    ),
    (
        "prepare_snipe_turn_off_all_tasks",
        "snipe_turn_off_all_tasks",
        "Prepare turning off all snipe tasks.",
        ToolSchema::Empty,
    ),
    (
        "prepare_snipe_turn_on_all_tasks",
        "snipe_turn_on_all_tasks",
        "Prepare turning on all snipe tasks.",
        ToolSchema::Empty,
    ),
    (
        "prepare_snipe_del_cfg",
        "snipe_del_cfg",
        "Prepare deleting a snipe task.",
        ToolSchema::Id,
    ),
    (
        "prepare_snipe_clear_all_cfgs",
        "snipe_clear_all_cfgs",
        "Prepare clearing all snipe tasks.",
        ToolSchema::Empty,
    ),
    (
        "prepare_snipe_set_fields",
        "snipe_set_fields",
        "Prepare raw SnipeUpdate fields.",
        ToolSchema::Raw,
    ),
    (
        "prepare_ct_new_cfg_def",
        "ct_new_cfg_def",
        "Prepare creating a default copy-trade task.",
        ToolSchema::Empty,
    ),
    (
        "prepare_ct_duplicate_cfg",
        "ct_duplicate_cfg",
        "Prepare duplicating a copy-trade task.",
        ToolSchema::Id,
    ),
    (
        "prepare_ct_turn_off_all_tasks",
        "ct_turn_off_all_tasks",
        "Prepare turning off all copy-trade tasks.",
        ToolSchema::Empty,
    ),
    (
        "prepare_ct_turn_on_all_tasks",
        "ct_turn_on_all_tasks",
        "Prepare turning on all copy-trade tasks.",
        ToolSchema::Empty,
    ),
    (
        "prepare_ct_del_cfg",
        "ct_del_cfg",
        "Prepare deleting a copy-trade task.",
        ToolSchema::Id,
    ),
    (
        "prepare_ct_clear_all_cfgs",
        "ct_clear_all_cfgs",
        "Prepare clearing all copy-trade tasks.",
        ToolSchema::Empty,
    ),
    (
        "prepare_ct_set_fields",
        "ct_set_fields",
        "Prepare raw CtUpdate fields.",
        ToolSchema::Raw,
    ),
    (
        "prepare_change_api_key",
        "change_api_key",
        "Prepare changing the Aura API key.",
        ToolSchema::Raw,
    ),
    (
        "prepare_switch_wallet",
        "switch_wallet",
        "Prepare switching active wallet.",
        ToolSchema::Address,
    ),
    (
        "prepare_remove_wallet",
        "remove_wallet",
        "Prepare removing a wallet.",
        ToolSchema::Raw,
    ),
    (
        "prepare_add_wallet",
        "add_wallet",
        "Prepare adding a wallet from a base58 keypair.",
        ToolSchema::AddWallet,
    ),
    (
        "prepare_wrap_wsol",
        "wrap_wsol",
        "Prepare wrapping SOL.",
        ToolSchema::Raw,
    ),
    (
        "prepare_unwrap_wsol",
        "unwrap_wsol",
        "Prepare unwrapping WSOL.",
        ToolSchema::Raw,
    ),
    (
        "prepare_open_ta",
        "open_ta",
        "Prepare opening a token account.",
        ToolSchema::Raw,
    ),
    (
        "prepare_open_util_accs",
        "open_util_accs",
        "Prepare opening utility accounts for a wallet.",
        ToolSchema::Address,
    ),
    (
        "prepare_make_withdraw",
        "make_withdraw",
        "Prepare a withdraw.",
        ToolSchema::Raw,
    ),
    (
        "prepare_create_nonces",
        "create_nonces",
        "Prepare creating nonces.",
        ToolSchema::Raw,
    ),
    (
        "prepare_update_nonces",
        "update_nonces",
        "Prepare updating nonces.",
        ToolSchema::Raw,
    ),
    (
        "prepare_dex_cu_set",
        "dex_cu_set",
        "Prepare updating DEX compute-unit settings.",
        ToolSchema::Raw,
    ),
];

pub fn tools() -> Vec<Value> {
    let mut tools = vec![
        tool(
            "get_aura_status",
            "Ping the configured Aura gRPC API.",
            empty_schema(),
        ),
        tool(
            "get_account_info",
            "Fetch active wallet, balances, and account counters.",
            empty_schema(),
        ),
        tool(
            "list_wallets",
            "List Aura wallets and utility account state.",
            empty_schema(),
        ),
        tool(
            "list_snipe_tasks",
            "List configured Aura sniper tasks.",
            empty_schema(),
        ),
        tool(
            "list_limit_orders",
            "List active Aura limit orders.",
            empty_schema(),
        ),
        tool(
            "get_bot_status",
            "Fetch compact bot/account status counters.",
            empty_schema(),
        ),
        tool(
            "start_user_activity",
            "Start Aura user_activity streaming with internal user_ping keepalive.",
            empty_schema(),
        ),
        tool(
            "read_user_activity",
            "Read buffered Aura user_activity events after a sequence cursor.",
            schema::<ReadActivityArgs>(),
        ),
        tool(
            "user_activity_status",
            "Return Aura user_activity stream state.",
            empty_schema(),
        ),
        tool(
            "stop_user_activity",
            "Stop Aura user_activity streaming and internal keepalive.",
            empty_schema(),
        ),
        tool(
            "explain_aura_error",
            "Explain a known Aura/API error and suggest next steps.",
            tool_schema(ToolSchema::Explain),
        ),
        tool(
            "prepare_limit_order",
            "Prepare raw Aura limit-order placement and return a confirmation id.",
            tool_schema(ToolSchema::Raw),
        ),
        tool(
            "confirm_limit_order",
            "Confirm a prepared limit order creation request.",
            tool_schema(ToolSchema::Confirmation),
        ),
        tool(
            "cancel_limit_order",
            "Prepare a limit order cancellation and return a confirmation id.",
            tool_schema(ToolSchema::CancelLimitOrder),
        ),
        tool(
            "confirm_mutation",
            "Execute a prepared mutation that uses a confirmation id.",
            tool_schema(ToolSchema::Confirmation),
        ),
        tool(
            "prepare_snipe_task",
            "Prepare creating a default sniper task.",
            tool_schema(ToolSchema::Empty),
        ),
        tool(
            "confirm_snipe_task",
            "Confirm a prepared sniper task creation request.",
            tool_schema(ToolSchema::Confirmation),
        ),
        tool(
            "update_snipe_task",
            "Prepare raw SnipeUpdate fields.",
            tool_schema(ToolSchema::Raw),
        ),
    ];
    tools.extend(
        READ_TOOLS
            .iter()
            .map(|(name, description, schema)| tool(name, description, tool_schema(*schema))),
    );
    tools.extend(
        MUTATION_TOOLS
            .iter()
            .map(|(name, _, description, schema)| tool(name, description, tool_schema(*schema))),
    );
    tools
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    let trading = is_trading_tool_name(name);
    let raw = is_raw_tool_name(name);
    let description = if raw {
        format!("{description}\n\n{RAW_REQUEST_NOTICE}")
    } else {
        description.to_owned()
    };
    let description = if trading {
        format!(
            "{description}\n\n{RATE_LIMIT_NOTICE}\n\n{TRADING_PREREQ_NOTICE}\n\n{BATCHING_NOTICE}"
        )
    } else {
        format!("{description}\n\n{RATE_LIMIT_NOTICE}\n\n{BATCHING_NOTICE}")
    };
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
        "_meta": {
            "aura_rate_limits": {
                "requests_per_second": 4,
                "requests_per_minute": 60,
                "scope": "per API key and per IP address",
                "recommended_sweep_delay_seconds": 0.5,
                "ban_threshold_requests_per_second": 10,
                "ban_threshold_requests_per_minute": 150,
                "ban_duration_hours": 24
            },
            "aura_trading_prerequisites": {
                "applies": trading,
                "instruction": TRADING_PREREQ_NOTICE
            },
            "aura_batching_recommendations": {
                "prefer_batching": true,
                "instruction": BATCHING_NOTICE,
                "trade": "Use MarketTrade.limit_orders instead of calling trade and then place_limit_orders when follow-up orders are known up front.",
                "snipe": "Use one SnipeUpdate with multiple updates entries instead of several snipe_set_fields calls.",
                "copy_trade": "Use one CtUpdate with multiple updates entries instead of several ct_set_fields calls.",
                "pubkeys": "Use ConfigPubkeys.pubkeys to update multiple wallets, mints, devs, or blacklist entries in one request."
            },
            "aura_mutation_flow": mutation_flow_meta(name),
            "aura_raw_request": raw_request_meta(name),
            "aura_argument_notes": argument_notes_meta(name)
        }
    })
}

fn mutation_flow_meta(name: &str) -> Value {
    if is_prepare_tool(name) || matches!(name, "cancel_limit_order") {
        json!({
            "prepare_tool": true,
            "executes_live_api_call": false,
            "confirm_with": "confirm_mutation",
            "response_field": "data.confirmation_id",
            "instruction": "Prepare tools only stage a mutation. Execute the returned confirmation_id with confirm_mutation or the matching confirm alias."
        })
    } else if matches!(
        name,
        "confirm_mutation" | "confirm_limit_order" | "confirm_snipe_task"
    ) {
        json!({
            "prepare_tool": false,
            "executes_live_api_call": true,
            "instruction": "Confirmation tools execute a previously prepared mutation and count against Aura rate limits."
        })
    } else {
        json!({
            "prepare_tool": false,
            "executes_live_api_call": false
        })
    }
}

fn raw_request_meta(name: &str) -> Value {
    if !is_raw_tool_name(name) {
        return Value::Null;
    }

    json!({
        "accepted_argument_forms": [
            "{\"request\": { ... }}",
            "{\"request\": \"{... JSON ...}\"}",
            "{ ... direct raw request object ... }"
        ],
        "common_rules": {
            "addresses": "Use base58 strings. The MCP server converts them to solana_address::Address where friendly wrappers are available.",
            "timedelta": "Use [seconds, nanoseconds], e.g. [30, 0].",
            "quote_lamports": "Use the Rust serde enum shape, e.g. {\"Buy\":{\"Lamports\":1000000}} for 0.001 SOL.",
            "ud128_percent": "Use ratios: \"1\" means 100%, \"0.5\" means 50%. Do not pass 100 for 100%.",
            "defaults": "prepare_trade, prepare_place_limit_orders, and prepare_limit_order fill slippage, tip, fee, processors, durable nonce, slot latency, and price-impact defaults when omitted."
        },
        "examples": raw_request_examples(name)
    })
}

fn raw_request_examples(name: &str) -> Value {
    match name {
        "prepare_trade" => json!({
            "market_buy_usdc_with_delayed_market_sell": {
                "request": {
                    "wallet": "AURAXd1nDoqtUDnjTFeedapcbSTid5XYhYpm2hhN6wd9",
                    "amount": { "Buy": { "Lamports": 1000000 } },
                    "mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                    "limit_orders": {
                        "orders": [{
                            "state": { "Api": { "id": null, "expire_dur": null, "activate_dur": [30, 0] } },
                            "order": {
                                "target": { "Market": { "mode": "Always" } },
                                "amount": { "SellPerc": { "amount": "1" } }
                            },
                            "trigger": "Immediate",
                            "wallet": "AURAXd1nDoqtUDnjTFeedapcbSTid5XYhYpm2hhN6wd9"
                        }]
                    }
                }
            }
        }),
        "prepare_place_limit_orders" | "prepare_limit_order" => json!({
            "delayed_market_buy_usdc": {
                "request": {
                    "mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                    "orders": {
                        "orders": [{
                            "state": { "Api": { "id": null, "expire_dur": null, "activate_dur": [60, 0] } },
                            "order": {
                                "target": { "Market": { "mode": "Always" } },
                                "amount": { "Buy": { "Lamports": 1000000 } }
                            },
                            "trigger": "Immediate",
                            "wallet": "AURAXd1nDoqtUDnjTFeedapcbSTid5XYhYpm2hhN6wd9"
                        }]
                    }
                }
            }
        }),
        "prepare_create_nonces" => json!({
            "create_one_durable_nonce": {
                "request": {
                    "wallet": "AURAXd1nDoqtUDnjTFeedapcbSTid5XYhYpm2hhN6wd9",
                    "amount": 1
                }
            }
        }),
        "prepare_open_ta" => json!({
            "open_usdc_pda": {
                    "request": {
                    "owner": "AURAXd1nDoqtUDnjTFeedapcbSTid5XYhYpm2hhN6wd9",
                    "mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                    "kind": "Pda",
                    "is_2022": false
                }
            }
        }),
        "prepare_dex_cu_set" => json!({
            "use_client_defaults": {
                "request": {}
            }
        }),
        _ => json!({}),
    }
}

fn argument_notes_meta(name: &str) -> Value {
    let mut notes = serde_json::Map::new();

    if matches!(
        name,
        "snipe_get_cfg"
            | "snipe_get_mints"
            | "snipe_get_devs"
            | "snipe_get_blacklist"
            | "snipe_cfg_get_limit_orders"
            | "snipe_cfg_get_buy_txn_proc"
            | "snipe_cfg_get_sell_txn_proc"
            | "prepare_snipe_duplicate_cfg"
            | "prepare_snipe_del_cfg"
    ) {
        notes.insert(
            "id".to_owned(),
            json!("Use an existing snipe task id returned by snipe_get_cfgs or list_snipe_tasks. Placeholder ids return not-found/permission errors."),
        );
    }

    if matches!(
        name,
        "ct_get_cfg"
            | "ct_get_copy_wallets"
            | "ct_get_buy_blacklist"
            | "ct_get_sell_blacklist"
            | "ct_cfg_get_limit_orders"
            | "ct_cfg_get_buy_txn_proc"
            | "ct_cfg_get_sell_txn_proc"
            | "prepare_ct_duplicate_cfg"
            | "prepare_ct_del_cfg"
    ) {
        notes.insert(
            "id".to_owned(),
            json!("Use an existing copy-trade task id returned by ct_get_cfgs. Placeholder ids return not-found/permission errors."),
        );
    }

    if matches!(name, "prepare_add_wallet") {
        notes.insert(
            "keypair_base58".to_owned(),
            json!("Full Solana keypair secret encoded as base58, normally 64 secret-key bytes encoded with bs58. Do not pass a wallet address/public key or random bytes."),
        );
    }

    if matches!(
        name,
        "confirm_mutation" | "confirm_limit_order" | "confirm_snipe_task"
    ) {
        notes.insert(
            "confirmation_id".to_owned(),
            json!("Use data.confirmation_id from the matching prepare response. Confirmation ids are local to this MCP process and expire after five minutes."),
        );
    }

    if notes.is_empty() {
        Value::Null
    } else {
        Value::Object(notes)
    }
}

fn is_raw_tool_name(name: &str) -> bool {
    matches!(name, "prepare_limit_order" | "update_snipe_task")
        || READ_TOOLS
            .iter()
            .any(|(tool, _, schema)| *tool == name && matches!(schema, ToolSchema::Raw))
        || MUTATION_TOOLS
            .iter()
            .any(|(tool, _, _, schema)| *tool == name && matches!(schema, ToolSchema::Raw))
}

fn is_trading_tool_name(name: &str) -> bool {
    matches!(
        name,
        "prepare_trade"
            | "prepare_limit_order"
            | "prepare_place_limit_orders"
            | "confirm_limit_order"
            | "cancel_limit_order"
            | "prepare_delete_limit_orders"
            | "prepare_clear_limit_orders"
            | "prepare_snipe_task"
            | "confirm_snipe_task"
            | "prepare_snipe_new_cfg_def"
            | "prepare_snipe_duplicate_cfg"
            | "prepare_snipe_turn_off_all_tasks"
            | "prepare_snipe_turn_on_all_tasks"
            | "prepare_snipe_del_cfg"
            | "prepare_snipe_clear_all_cfgs"
            | "prepare_snipe_set_fields"
            | "update_snipe_task"
            | "prepare_ct_new_cfg_def"
            | "prepare_ct_duplicate_cfg"
            | "prepare_ct_turn_off_all_tasks"
            | "prepare_ct_turn_on_all_tasks"
            | "prepare_ct_del_cfg"
            | "prepare_ct_clear_all_cfgs"
            | "prepare_ct_set_fields"
            | "confirm_mutation"
    )
}

pub fn resources() -> Vec<Value> {
    RESOURCE_URIS
        .iter()
        .map(|(uri, name, description)| {
            json!({
                "uri": uri,
                "name": name,
                "description": description,
                "mimeType": if *uri == ACTIVITY_URI { "application/json" } else { "text/markdown" }
            })
        })
        .collect()
}

const RESOURCE_URIS: &[(&str, &str, &str)] = &[
    (
        ACTIVITY_URI,
        "Aura User Activity Latest",
        "Latest Aura user_activity event snapshot.",
    ),
    (
        "aura://docs/overview",
        "Aura Overview",
        "Local overview for the Aura MCP server.",
    ),
    (
        "aura://docs/auth",
        "Aura Auth",
        "API key and local config notes.",
    ),
    (
        "aura://docs/grpc",
        "Aura gRPC",
        "How this MCP server calls the Aura gRPC API.",
    ),
    (
        "aura://docs/tools",
        "Aura MCP Tools",
        "Tool behavior, safety, and read-only mode.",
    ),
    (
        "aura://docs/api",
        "Aura API Methods",
        "Method and payload reference for Aura MCP and the Rust client.",
    ),
    (
        "aura://instructions/agent",
        "Aura Agent Instructions",
        "Short system-prompt instructions for agents using Aura MCP.",
    ),
    (
        "aura://proto/main",
        "Aura Main Proto",
        "Pointer to the canonical Aura core gRPC proto definition.",
    ),
    (
        "aura://examples/rust",
        "Aura Rust Example",
        "Rust client usage example.",
    ),
    (
        "aura://examples/typescript",
        "Aura TypeScript Example",
        "TypeScript gRPC usage sketch.",
    ),
];

fn resource_text(uri: &str) -> Option<&'static str> {
    match uri {
        "aura://docs/overview" => Some(include_str!("../docs/overview.md")),
        "aura://docs/auth" => Some(include_str!("../docs/auth.md")),
        "aura://docs/grpc" => Some(include_str!("../docs/grpc.md")),
        "aura://docs/tools" => Some(include_str!("../docs/tools.md")),
        "aura://docs/api" => Some(include_str!("../docs/api.md")),
        "aura://instructions/agent" => Some(include_str!("../docs/agent-instructions.md")),
        "aura://proto/main" => Some(include_str!("../docs/proto-main.md")),
        "aura://examples/rust" => Some(include_str!("../examples/rust.md")),
        "aura://examples/typescript" => Some(include_str!("../examples/typescript.md")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_blocks_mutations() {
        assert!(read_only_blocks_mutation(&Config::default()));
        assert!(!read_only_blocks_mutation(&Config {
            read_only: false,
            ..Config::default()
        }));
    }

    #[test]
    fn rpc_response_is_single_json_line() {
        let mut out = Vec::new();
        write_message(&mut out, &rpc_result(Some(json!(1)), json!({"ok": true}))).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.lines().count(), 1);
        let parsed: Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
    }

    #[test]
    fn all_non_streaming_client_methods_are_exposed() {
        let names = tools()
            .into_iter()
            .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
            .collect::<std::collections::HashSet<_>>();

        for name in [
            "aura_user_ping",
            "prepare_trade",
            "fetch_state_info",
            "get_token_status",
            "get_token_most_liq_pool",
            "get_token_meta",
            "get_token_trade_stats",
            "get_token_positions",
            "get_token_positions_ui",
            "fetch_full_wallet_info",
            "get_token_limit_orders",
            "get_limit_orders",
            "start_user_activity",
            "read_user_activity",
            "user_activity_status",
            "stop_user_activity",
            "prepare_place_limit_orders",
            "prepare_delete_limit_orders",
            "prepare_clear_limit_orders",
            "prepare_snipe_new_cfg_def",
            "prepare_snipe_duplicate_cfg",
            "prepare_snipe_turn_off_all_tasks",
            "prepare_snipe_turn_on_all_tasks",
            "prepare_snipe_del_cfg",
            "prepare_snipe_clear_all_cfgs",
            "snipe_get_cfgs",
            "snipe_get_cfg",
            "snipe_get_mints",
            "snipe_get_devs",
            "snipe_get_blacklist",
            "snipe_cfg_get_limit_orders",
            "snipe_cfg_get_buy_txn_proc",
            "snipe_cfg_get_sell_txn_proc",
            "prepare_snipe_set_fields",
            "ct_get_buy_blacklist",
            "ct_get_sell_blacklist",
            "prepare_ct_new_cfg_def",
            "prepare_ct_duplicate_cfg",
            "prepare_ct_turn_off_all_tasks",
            "prepare_ct_turn_on_all_tasks",
            "prepare_ct_del_cfg",
            "prepare_ct_clear_all_cfgs",
            "ct_get_cfgs",
            "ct_get_cfg",
            "ct_get_copy_wallets",
            "ct_cfg_get_limit_orders",
            "ct_cfg_get_buy_txn_proc",
            "ct_cfg_get_sell_txn_proc",
            "prepare_ct_set_fields",
            "prepare_change_api_key",
            "txn_procs_stat",
            "prepare_switch_wallet",
            "prepare_remove_wallet",
            "prepare_add_wallet",
            "prepare_wrap_wsol",
            "prepare_unwrap_wsol",
            "prepare_open_ta",
            "prepare_open_util_accs",
            "prepare_make_withdraw",
            "prepare_create_nonces",
            "prepare_update_nonces",
            "prepare_dex_cu_set",
            "dex_cu_get",
        ] {
            assert!(names.contains(name), "missing MCP tool for {name}");
        }
    }

    #[test]
    fn tools_expose_rate_limits_and_trading_prereqs() {
        for tool in tools() {
            let description = tool["description"].as_str().unwrap();
            assert!(description.contains("4 API requests/second"));
            assert!(description.contains("Batch when the API supports it"));
            assert_eq!(
                tool["_meta"]["aura_rate_limits"]["requests_per_second"],
                json!(4)
            );
            assert_eq!(
                tool["_meta"]["aura_batching_recommendations"]["prefer_batching"],
                json!(true)
            );
        }

        let trade = tools()
            .into_iter()
            .find(|tool| tool["name"] == "prepare_trade")
            .unwrap();
        assert!(
            trade["description"]
                .as_str()
                .unwrap()
                .contains("durable nonce")
        );
        assert_eq!(
            trade["_meta"]["aura_trading_prerequisites"]["applies"],
            json!(true)
        );
    }

    #[test]
    fn raw_tools_expose_payload_contract_and_examples() {
        let trade = tools()
            .into_iter()
            .find(|tool| tool["name"] == "prepare_trade")
            .unwrap();

        assert!(
            trade["description"]
                .as_str()
                .unwrap()
                .contains("JSON string")
        );
        assert!(trade["inputSchema"]["anyOf"].is_array());
        assert_eq!(
            trade["_meta"]["aura_raw_request"]["accepted_argument_forms"][0],
            json!("{\"request\": { ... }}")
        );
        assert_eq!(
            trade["_meta"]["aura_raw_request"]["examples"]["market_buy_usdc_with_delayed_market_sell"]
                ["request"]["amount"]["Buy"]["Lamports"],
            json!(1_000_000)
        );
        assert_eq!(
            trade["_meta"]["aura_mutation_flow"]["confirm_with"],
            json!("confirm_mutation")
        );
    }

    #[test]
    fn tools_expose_argument_source_notes() {
        let tools = tools();
        let add_wallet = tools
            .iter()
            .find(|tool| tool["name"] == "prepare_add_wallet")
            .unwrap();
        assert!(
            add_wallet["_meta"]["aura_argument_notes"]["keypair_base58"]
                .as_str()
                .unwrap()
                .contains("keypair secret")
        );

        let ct_cfg = tools
            .iter()
            .find(|tool| tool["name"] == "ct_get_cfg")
            .unwrap();
        assert!(
            ct_cfg["_meta"]["aura_argument_notes"]["id"]
                .as_str()
                .unwrap()
                .contains("ct_get_cfgs")
        );

        let confirm = tools
            .iter()
            .find(|tool| tool["name"] == "confirm_mutation")
            .unwrap();
        assert!(
            confirm["_meta"]["aura_argument_notes"]["confirmation_id"]
                .as_str()
                .unwrap()
                .contains("data.confirmation_id")
        );
    }

    #[test]
    fn mutation_prepare_returns_confirmation_when_writable() {
        let mut server = McpServer::new(Config {
            read_only: false,
            ..Config::default()
        });
        let result = server
            .prepare_api_mutation("prepare_snipe_task", json!({}))
            .unwrap();
        assert_eq!(result["ok"], true);
        assert!(result["data"]["confirmation_id"].as_str().is_some());
    }

    #[test]
    fn prepare_trade_validates_payload_before_confirmation() {
        let mut server = McpServer::new(Config {
            read_only: false,
            ..Config::default()
        });
        let err = server
            .prepare_api_mutation("prepare_trade", json!({"request": "{}"}))
            .unwrap_err()
            .to_string();

        assert!(err.contains("invalid mutation request"));
    }

    #[tokio::test]
    async fn activity_resource_can_be_read_and_subscribed() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let server = McpServer::new_with_output(Config::default(), tx);

        let read = server
            .read_resource(Some(json!({ "uri": ACTIVITY_URI })))
            .await
            .unwrap();
        assert_eq!(read["contents"][0]["mimeType"], "application/json");

        server
            .subscribe_resource(Some(json!({ "uri": ACTIVITY_URI })))
            .await
            .unwrap();
        assert!(server.activity.status().await.subscribed);
        server
            .activity
            .push_event_for_test(json!({"event": "test"}))
            .await;
        let notification = rx.try_recv().unwrap();
        assert_eq!(notification["method"], "notifications/resources/updated");

        server
            .unsubscribe_resource(Some(json!({ "uri": ACTIVITY_URI })))
            .await
            .unwrap();
        assert!(!server.activity.status().await.subscribed);
    }

    #[test]
    fn stdio_modules_do_not_log_to_stdout() {
        let stdout_macro = concat!("print", "ln!(");
        let debug_macro = concat!("d", "bg!(");
        for source in [
            include_str!("mcp.rs"),
            include_str!("aura.rs"),
            include_str!("activity.rs"),
        ] {
            assert!(!source.contains(stdout_macro));
            assert!(!source.contains(debug_macro));
        }
    }
}
