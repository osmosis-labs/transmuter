use cosmwasm_std::StdError;
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

    #[error("Too many denoms to transmute")]
    TooManyCoinsToTransmute {},

    #[error("Unable to supply coin with denom: {denom}: expected: {expected_denom}")]
    UnableToSupply {
        denom: String,
        expected_denom: String,
    },
}
