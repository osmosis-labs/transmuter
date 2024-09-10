use cosmwasm_std::{CheckedFromRatioError, DivideByZeroError, OverflowError};

#[derive(thiserror::Error, Debug, PartialEq)]
pub enum TransmuterMathError {
    /// Time invariant error, this should never happen
    #[error("Time must be monotonically increasing")]
    NonMonotonicTime,

    #[error("Moving average is undefined due to zero elapsed time since limiter started tracking")]
    UndefinedMovingAverage,

    #[error("Missing data points to calculate moving average")]
    MissingDataPoints,

    #[error("{0}")]
    OverflowError(#[from] OverflowError),

    #[error("{0}")]
    DivideByZeroError(#[from] DivideByZeroError),

    #[error("{0}")]
    CheckedFromRatioError(#[from] CheckedFromRatioError),
}
