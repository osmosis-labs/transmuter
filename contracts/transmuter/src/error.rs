use cosmwasm_std::{Coin, StdError, Uint128};
use thiserror::Error;

#[derive(Error, Debug, PartialEq)]
pub enum ContractError {
    #[error("{0}")]
    Std(#[from] StdError),

    #[error("Unauthorized")]
    Unauthorized {},

    #[error("Custom Error val: {val:?}")]
    CustomError { val: String },

    #[error("Denom not allowed: {denom}")]
    DenomNotAllowed { denom: String },

    #[error("Funds must contain exactly one token")]
    SingleTokenExpected {},

    #[error("Funds must contain at least one token")]
    AtLeastSingleTokenExpected {},

    #[error("Unable to join pool with denom: {denom}: expected one of: {expected_denom:?}")]
    InvalidJoinPoolDenom {
        denom: String,
        expected_denom: Vec<String>,
    },

    #[error("Unable to transmute token with denom: {denom}: expected one of: {expected_denom:?}")]
    InvalidTransmuteDenom {
        denom: String,
        expected_denom: Vec<String>,
    },

    #[error("Insufficient fund: required: {required}, available: {available}")]
    InsufficientFund { required: Coin, available: Coin },

    #[error("Insufficient shares: required: {required}, available: {available}")]
    InsufficientShares {
        required: Uint128,
        available: Uint128,
    },
}
