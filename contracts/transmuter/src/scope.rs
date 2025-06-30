use cosmwasm_schema::cw_serde;
use std::{fmt::Display, str::FromStr};

/// Scope for configuring rebalancer
#[cw_serde]
#[serde(tag = "type", content = "value")]
#[derive(Eq, Hash)]
pub enum Scope {
    Denom(String),
    AssetGroup(String),
}

impl Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.key())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Invalid scope: {0}, must start with 'denom::' or 'asset_group::'")]
pub struct ParseScopeErr(String);

impl FromStr for Scope {
    type Err = ParseScopeErr;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.starts_with("denom::") {
            s.strip_prefix("denom::")
                .map(|s| Scope::Denom(s.to_string()))
                .ok_or(ParseScopeErr(s.to_string()))
        } else if s.starts_with("asset_group::") {
            s.strip_prefix("asset_group::")
                .map(|s| Scope::AssetGroup(s.to_string()))
                .ok_or(ParseScopeErr(s.to_string()))
        } else {
            Err(ParseScopeErr(s.to_string()))
        }
    }
}

impl Scope {
    pub fn key(&self) -> String {
        match self {
            Scope::Denom(denom) => format!("denom::{}", denom),
            Scope::AssetGroup(label) => format!("asset_group::{}", label),
        }
    }
}

impl Scope {
    pub fn denom(denom: &str) -> Self {
        Scope::Denom(denom.to_string())
    }

    pub fn asset_group(label: &str) -> Self {
        Scope::AssetGroup(label.to_string())
    }
}
