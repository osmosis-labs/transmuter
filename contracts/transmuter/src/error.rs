use cosmwasm_std::{Coin, Decimal, StdError, Uint128};
use cw_controllers::AdminError;
use thiserror::Error;

#[derive(Error, Debug, PartialEq)]
pub enum ContractError {
    #[error("{0}")]
    Std(#[from] StdError),

    #[error("{0}")]
    Admin(#[from] AdminError),

    #[error("Funds must contain exactly one token")]
    SingleTokenExpected {},

    #[error("Funds must contain at least one token")]
    AtLeastSingleTokenExpected {},

    #[error("Unable to join pool with denom: {denom}: expected one of: {expected_denom:?}")]
    InvalidJoinPoolDenom {
        denom: String,
        expected_denom: Vec<String>,
    },

    #[error("Unable to transmute token with denom: {denom}: expected one of: {expected_denom:?} or share token")]
    InvalidTransmuteDenom {
        denom: String,
        expected_denom: Vec<String>,
    },

    #[error("Insufficient pool asset: required: {required}, available: {available}")]
    InsufficientPoolAsset { required: Coin, available: Coin },

    #[error("Funds mismatch token in: funds: {funds:?}, token_in: {token_in}")]
    FundsMismatchTokenIn { funds: Vec<Coin>, token_in: Coin },

    #[error("Insufficient shares: required: {required}, available: {available}")]
    InsufficientShares {
        required: Uint128,
        available: Uint128,
    },

    #[error("Invalid swap fee: expected: {expected}, actual: {actual}")]
    InvalidSwapFee { expected: Decimal, actual: Decimal },

    /// This error should never occur, but is here for completeness
    /// This will happens if and only if calculated token out and expected token out are not equal
    #[error("Invalid token out amount: expected: {expected}, actual: {actual}")]
    InvalidTokenOutAmount { expected: Uint128, actual: Uint128 },

    #[error("Spot price query failed: reason {reason}")]
    SpotPriceQueryFailed { reason: String },

    #[error("Insufficient token out: required: {required}, available: {available}")]
    InsufficientTokenOut {
        required: Uint128,
        available: Uint128,
    },

    #[error("Excessive token in required: max acceptable token in: {limit}, required: {required}")]
    ExcessiveRequiredTokenIn { limit: Uint128, required: Uint128 },

    #[error("The pool is currently inactive")]
    InactivePool {},

    #[error("YUnexpected denom: expected: {expected}, actual: {actual}")]
    UnexpectedDenom { expected: String, actual: String },
}
