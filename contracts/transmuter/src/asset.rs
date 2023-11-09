use cosmwasm_schema::cw_serde;
use cosmwasm_std::Uint128;

#[cw_serde]
pub struct AssetConfig {
    pub denom: String,
    pub normalization_factor: Uint128,
}

impl AssetConfig {
    pub fn from_denom_str(denom: &str) -> Self {
        Self {
            denom: denom.to_string(),
            normalization_factor: Uint128::one(),
        }
    }
}
