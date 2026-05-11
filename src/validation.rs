use anyhow::{Result, anyhow};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use solana_address::Address;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TradeSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AmountKind {
    Quote,
    Percent,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrepareLimitOrderArgs {
    pub mint: String,
    pub side: TradeSide,
    pub amount: f64,
    pub amount_kind: AmountKind,
    pub slippage_bps: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfirmationArgs {
    pub confirmation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelLimitOrderArgs {
    pub mint: String,
    #[serde(default)]
    pub order_ids: Vec<i64>,
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExplainAuraErrorArgs {
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddressArg {
    pub address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OptionalMintArg {
    #[serde(default)]
    pub mint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdArg {
    /// Existing Aura task/config id returned by the matching list tool.
    pub id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawRequestArg {
    pub request: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddWalletArg {
    /// Full Solana keypair secret encoded as base58. This is not a wallet address/public key.
    pub keypair_base58: String,
}

pub fn parse_address(value: &str, field: &str) -> Result<Address> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("{field} must not be empty"));
    }
    Address::from_str(trimmed).map_err(|_| anyhow!("{field} is not a valid Solana address"))
}

pub fn validate_prepare_limit_order(args: &PrepareLimitOrderArgs) -> Result<()> {
    parse_address(&args.mint, "mint")?;
    if !args.amount.is_finite() || args.amount <= 0.0 {
        return Err(anyhow!("amount must be a positive finite number"));
    }
    if matches!(args.amount_kind, AmountKind::Percent) && args.amount > 100.0 {
        return Err(anyhow!("percent amount must be <= 100"));
    }
    if args.slippage_bps > 10_000 {
        return Err(anyhow!("slippage_bps must be between 0 and 10000"));
    }
    Ok(())
}

pub fn validate_cancel_limit_order(args: &CancelLimitOrderArgs) -> Result<()> {
    parse_address(&args.mint, "mint")?;
    if args.all && !args.order_ids.is_empty() {
        return Err(anyhow!("use either all=true or order_ids, not both"));
    }
    if !args.all && args.order_ids.is_empty() {
        return Err(anyhow!("order_ids is required unless all=true"));
    }
    if args.order_ids.iter().any(|id| *id <= 0) {
        return Err(anyhow!("order_ids must be positive"));
    }
    Ok(())
}

pub fn validate_id(id: i64, field: &str) -> Result<()> {
    if id <= 0 {
        return Err(anyhow!("{field} must be positive"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    #[test]
    fn validates_limit_order_args() {
        let args = PrepareLimitOrderArgs {
            mint: MINT.into(),
            side: TradeSide::Buy,
            amount: 1.0,
            amount_kind: AmountKind::Quote,
            slippage_bps: 100,
        };
        validate_prepare_limit_order(&args).unwrap();

        let mut bad = args.clone();
        bad.amount = 0.0;
        assert!(validate_prepare_limit_order(&bad).is_err());

        bad = args.clone();
        bad.slippage_bps = 10_001;
        assert!(validate_prepare_limit_order(&bad).is_err());
    }

    #[test]
    fn validates_cancel_args() {
        validate_cancel_limit_order(&CancelLimitOrderArgs {
            mint: MINT.into(),
            order_ids: vec![1, 2],
            all: false,
        })
        .unwrap();

        assert!(
            validate_cancel_limit_order(&CancelLimitOrderArgs {
                mint: MINT.into(),
                order_ids: vec![],
                all: false,
            })
            .is_err()
        );
    }
}
