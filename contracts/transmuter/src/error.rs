use cosmwasm_std::{Coin, Decimal, StdError, Uint128};
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

    #[error("Invalid swap fee: expected: {expected}, actual: {actual}")]
    InvalidSwapFee { expected: Decimal, actual: Decimal },

    /// Error when >= requirement is not met
    #[error("Insufficient token out: required: {required}, available: {available}")]
    InsufficientTokenOut {
        required: Uint128,
        available: Uint128,
    },

    /// Error when == requirement is not met
    #[error("Invalid token out amount: expected: {expected}, actual: {actual}")]
    InvalidTokenOutAmount { expected: Uint128, actual: Uint128 },

    #[error("Spot price query failed: reason {reason}")]
    SpotPriceQueryFailed { reason: String },

    #[error("Exceeding token in required: max acceptable token in: {limit}, required: {required}")]
    ExceedingTokenIn { limit: Uint128, required: Uint128 },

    #[error("The pool is currently inactive")]
    InactivePool {},
}
