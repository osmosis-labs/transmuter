use cosmwasm_std::{
    CheckedFromRatioError, CheckedMultiplyRatioError, Coin, ConversionOverflowError, Decimal,
    DecimalRangeExceeded, DivideByZeroError, OverflowError, StdError, Timestamp, Uint128, Uint64,
};
use thiserror::Error;

use crate::{math::MathError, scope::Scope};

#[derive(Error, Debug, PartialEq)]
pub enum ContractError {
    #[error("{0}")]
    Std(#[from] StdError),

    #[error("{0}")]
    VersionError(#[from] cw2::VersionError),

    #[error("`{field}` must not be empty")]
    NonEmptyInputRequired { field: String },

    #[error("Funds must be empty")]
    Nonpayable {},

    #[error("Funds must contain at least one token")]
    AtLeastSingleTokenExpected {},

    #[error("Denom has no supply, it might be an invalid denom: {denom}")]
    DenomHasNoSupply { denom: String },

    #[error("Subdenom must not contain extra parts (separated by '/'): {subdenom}")]
    SubDenomExtraPartsNotAllowed { subdenom: String },

    #[error("Unable to join pool with denom: {denom}: expected one of: {expected_denom:?}")]
    InvalidJoinPoolDenom {
        denom: String,
        expected_denom: Vec<String>,
    },

    #[error("Unable to transmute token with denom: {denom}: expected one of: {expected_denom:?} or alloyed asset")]
    InvalidTransmuteDenom {
        denom: String,
        expected_denom: Vec<String>,
    },

    #[error("Not a pool asset denom: {denom}")]
    InvalidPoolAssetDenom { denom: String },

    #[error("Not a corrupted asset denom: {denom}")]
    InvalidCorruptedAssetDenom { denom: String },

    #[error("Only corrupted asset with 0 amount can be removed")]
    InvalidAssetRemoval {},

    #[error("Pool asset denom count must be within {min} - {max} inclusive, but got: {actual}")]
    PoolAssetDenomCountOutOfRange {
        min: Uint64,
        max: Uint64,
        actual: Uint64,
    },

    #[error("Asset group count must be within {max} inclusive, but got: {actual}")]
    AssetGroupCountOutOfRange { max: Uint64, actual: Uint64 },

    #[error("Insufficient pool asset: required: {required}, available: {available}")]
    InsufficientPoolAsset { required: Coin, available: Coin },

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

    /// This error should never occur, but is here for completeness
    /// This will happens if and only if calculated token in and expected token in are not equal
    #[error("Invalid token in amount: expected: {expected}, actual: {actual}")]
    InvalidTokenInAmount { expected: Uint128, actual: Uint128 },

    #[error("Spot price query failed: reason {reason}")]
    SpotPriceQueryFailed { reason: String },

    #[error(
        "Insufficient token out: min required: {min_required}, but got calculated amount out: {amount_out}"
    )]
    InsufficientTokenOut {
        min_required: Uint128,
        amount_out: Uint128,
    },

    #[error("Excessive token in required: max acceptable token in: {limit}, required: {required}")]
    ExcessiveRequiredTokenIn { limit: Uint128, required: Uint128 },

    #[error("The pool is currently inactive")]
    InactivePool {},

    #[error("Attempt to set pool to active status to {status} when it is already {status}")]
    UnchangedActiveStatus { status: bool },

    #[error("Duplicated pool asset denom: {denom}")]
    DuplicatedPoolAssetDenom { denom: String },

    #[error("Pool asset not be share denom")]
    ShareDenomNotAllowedAsPoolAsset {},

    #[error("Token in must not have the same denom as token out: {denom}")]
    SameDenomNotAllowed { denom: String },

    #[error("Unauthorized")]
    Unauthorized {},

    #[error("Admin transferring state is inoperable for the requested operation")]
    InoperableAdminTransferringState {},

    #[error("Limiter count for {scope} exceed maximum per denom: {max}")]
    MaxLimiterCountPerDenomExceeded { scope: Scope, max: Uint64 },

    #[error("Denom: {scope} cannot have an empty limiter after it has been registered")]
    EmptyLimiterNotAllowed { scope: Scope },

    #[error("Limiter label must not be empty")]
    EmptyLimiterLabel {},

    #[error("Amount of coin to be operated on must be greater than zero")]
    ZeroValueOperation {},

    #[error("Window size must be greater than zero")]
    ZeroWindowSize {},

    #[error("Boundary must be greater than zero")]
    ZeroBoundaryOffset {},

    #[error("Upper limit must be greater than zero")]
    ZeroUpperLimit {},

    #[error("Upper limit must not exceed 100%")]
    ExceedHundredPercentUpperLimit {},

    #[error("Window must be evenly divisible by division size")]
    UnevenWindowDivision {},

    #[error("Division count must not exceed {max_division_count}")]
    DivisionCountExceeded { max_division_count: Uint64 },

    #[error("Moving average is undefined due to zero elapsed time since limiter started tracking")]
    UndefinedMovingAverage {},

    /// Time invariant error, this should never happen
    #[error("Time must be monotonically increasing")]
    NonMonotonicTime {},

    /// Time invariant error, this should never happen
    #[error("Division's update should occur before division ended: updated_at: {updated_at}, ended_at: {ended_at}")]
    UpdateAfterDivisionEnded {
        updated_at: Timestamp,
        ended_at: Timestamp,
    },

    #[error("Limiter does not exist for scope: {scope}, label: {label}")]
    LimiterDoesNotExist { scope: Scope, label: String },

    #[error("Limiter already exists for scope: {scope}, label: {label}")]
    LimiterAlreadyExists { scope: Scope, label: String },

    #[error(
        "Upper limit exceeded for `{scope}`, upper limit is {upper_limit}, but the resulted weight is {value}"
    )]
    UpperLimitExceeded {
        scope: Scope,
        upper_limit: Decimal,
        value: Decimal,
    },

    #[error("Modifying wrong limiter type: expected: {expected}, actual: {actual}")]
    WrongLimiterType { expected: String, actual: String },

    #[error("Normalization factor must be positive")]
    NormalizationFactorMustBePositive {},

    #[error("Corrupted scope: {scope} must not increase in amount or weight")]
    CorruptedScopeRelativelyIncreased { scope: Scope },

    #[error("Not a registered scope: {scope}")]
    InvalidScope { scope: Scope },

    // TODO: use error from transmuter_math instead
    #[error("Invalid lambda: {lambda}")]
    InvalidLambda { lambda: Decimal },

    #[error("Asset group {label} not found")]
    AssetGroupNotFound { label: String },

    #[error("Asset group {label} already exists")]
    AssetGroupAlreadyExists { label: String },

    #[error("Asset group label must not be empty")]
    EmptyAssetGroupLabel {},

    #[error("{0}")]
    OverflowError(#[from] OverflowError),

    #[error("{0}")]
    DivideByZeroError(#[from] DivideByZeroError),

    #[error("{0}")]
    CheckedFromRatioError(#[from] CheckedFromRatioError),

    #[error("{0}")]
    CheckedMultiplyRatioError(#[from] CheckedMultiplyRatioError),

    #[error("{0}")]
    ConversionOverflowError(#[from] ConversionOverflowError),

    #[error("{0}")]
    DecimalRangeExceeded(#[from] DecimalRangeExceeded),
    #[error("{0}")]
    MathError(#[from] MathError),

    #[error("{0}")]
    TrasnmuterMathError(#[from] transmuter_math::TransmuterMathError),

    /// This error should never occur
    #[error("")]
    Never,
}

pub fn nonpayable(funds: &[Coin]) -> Result<(), ContractError> {
    if funds.is_empty() {
        Ok(())
    } else {
        Err(ContractError::Nonpayable {})
    }
}

pub fn non_empty_input_required<T>(field_name: &str, value: &[T]) -> Result<(), ContractError> {
    if value.is_empty() {
        Err(ContractError::NonEmptyInputRequired {
            field: field_name.to_string(),
        })
    } else {
        Ok(())
    }
}
