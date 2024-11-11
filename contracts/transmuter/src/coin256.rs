use std::fmt::{self, Display};

use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Coin, Uint256};

#[cw_serde]
pub struct Coin256 {
    pub amount: Uint256,
    pub denom: String,
}

impl Display for Coin256 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}{}", self.amount, self.denom)
    }
}

impl Coin256 {
    pub fn new(amount: impl Into<Uint256>, denom: impl Into<String>) -> Self {
        Self {
            amount: amount.into(),
            denom: denom.into(),
        }
    }

    pub fn zero(denom: impl Into<String>) -> Self {
        Self::new(0u128, denom)
    }
}

pub fn coin256(amount: u128, denom: impl Into<String>) -> Coin256 {
    Coin256::new(amount, denom)
}

impl From<Coin> for Coin256 {
    fn from(c: Coin) -> Self {
        Coin256::new(c.amount, c.denom)
    }
}
