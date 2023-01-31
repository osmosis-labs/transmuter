use cosmwasm_std::{Coin, StdError};
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

    #[error("Funds must contain exactly one coin")]
    SingleCoinExpected {},

    #[error("Unable to supply coin with denom: {denom}: expected: {expected_denom}")]
    InvalidSupplyDenom {
        denom: String,
        expected_denom: String,
    },

    #[error("Unable to transmute coin with denom: {denom}: expected: {expected_denom}")]
    InvalidTransmuteDenom {
        denom: String,
        expected_denom: String,
    },

    #[error("Insufficient fund: required: {required}, available: {available}")]
    InsufficientFund { required: Coin, available: Coin },
}
