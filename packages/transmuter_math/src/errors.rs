use cosmwasm_std::{
    CheckedFromRatioError, Decimal256RangeExceeded, DecimalRangeExceeded, DivideByZeroError,
    OverflowError,
};

#[derive(thiserror::Error, Debug, PartialEq)]
pub enum TransmuterMathError {
    /// Time invariant error, this should never happen
    #[error("Time must be monotonically increasing")]
    NonMonotonicTime,

    #[error("Moving average is undefined due to zero elapsed time since limiter started tracking")]
    UndefinedMovingAverage,

    #[error("Missing data points to calculate moving average")]
    MissingDataPoints,

    #[error("`{var_name}` must be within normalized range [0, 1]")]
    OutOfNormalizedRange { var_name: String },

    #[error("{0}")]
    DecimalRangeExceeded(#[from] DecimalRangeExceeded),

    #[error("{0}")]
    Decimal256RangeExceeded(#[from] Decimal256RangeExceeded),

    #[error("{0}")]
    OverflowError(#[from] OverflowError),

    #[error("{0}")]
    DivideByZeroError(#[from] DivideByZeroError),

    #[error("{0}")]
    CheckedFromRatioError(#[from] CheckedFromRatioError),
}
