use schemars::JsonSchema;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt::Display, str::FromStr};

/// Scope for configuring limiters & rebalacing incentive for
#[derive(Clone, Debug, PartialEq, JsonSchema, Eq, Hash, Ord, PartialOrd)]
#[serde(tag = "type", content = "value")]
pub enum Scope {
    Denom(String),
    AssetGroup(String),
}

impl Serialize for Scope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.key())
    }
}

impl<'de> Deserialize<'de> for Scope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Scope::from_str(&s).map_err(de::Error::custom)
    }
}

impl Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.key())
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
#[error("Invalid scope: {0}, must start with 'denom::' or 'asset_group::'")]
pub struct ParseScopeError(String);

impl FromStr for Scope {
    type Err = ParseScopeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.starts_with("denom::") {
            s.strip_prefix("denom::")
                .map(|s| Scope::Denom(s.to_string()))
                .ok_or(ParseScopeError(s.to_string()))
        } else if s.starts_with("asset_group::") {
            s.strip_prefix("asset_group::")
                .map(|s| Scope::AssetGroup(s.to_string()))
                .ok_or(ParseScopeError(s.to_string()))
        } else {
            Err(ParseScopeError(s.to_string()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::{from_json, to_json_string};

    #[test]
    fn test_serialize_deserialize_scope() {
        let scope_denom = Scope::denom("uusdc");
        let scope_asset_group = Scope::asset_group("group1");

        // Serialize to JSON string
        let json_denom = to_json_string(&scope_denom).unwrap();
        let json_asset_group = to_json_string(&scope_asset_group).unwrap();

        // assert json string is valid
        assert_eq!(json_denom, r#""denom::uusdc""#);
        assert_eq!(json_asset_group, r#""asset_group::group1""#);

        // Deserialize back from JSON string
        let deserialized_denom: Scope = from_json(json_denom.as_bytes()).unwrap();
        let deserialized_asset_group: Scope = from_json(json_asset_group.as_bytes()).unwrap();

        // Assert equality
        assert_eq!(scope_denom, deserialized_denom);
        assert_eq!(scope_asset_group, deserialized_asset_group);
    }
}
