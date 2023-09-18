use cosmwasm_std::{
    testing::{mock_dependencies, mock_env, mock_info},
    Coin, Decimal,
};

use crate::{contract::Transmuter, ContractError};

#[test]
fn test_spot_price_on_balanced_liquidity_must_be_one() {
    test_spot_price(&[Coin::new(100_000, "denom0"), Coin::new(100_000, "denom1")])
}
#[test]
fn test_spot_price_on_unbalanced_liquidity_must_be_one() {
    test_spot_price(&[
        Coin::new(999_999_999, "denom0"),
        Coin::new(100_000, "denom1"),
    ])
}

fn test_spot_price(liquidity: &[Coin]) {
    let transmuter = Transmuter::new();
    let mut deps = mock_dependencies();

    // make denom has non-zero total supply
    deps.querier.update_balance(
        "someone",
        vec![Coin::new(1, "denom0"), Coin::new(1, "denom1")],
    );

    transmuter
        .instantiate(
            (deps.as_mut(), mock_env(), mock_info("creator", &[])),
            vec!["denom0".to_string(), "denom1".to_string()],
            "transmuter/poolshare".to_string(),
            None,
        )
        .unwrap();

    transmuter
        .alloyed_asset
        .set_alloyed_denom(
            &mut deps.storage,
            &"factory/contract_address/transmuter/poolshare".to_string(),
        )
        .unwrap();

    transmuter
        .join_pool((deps.as_mut(), mock_env(), mock_info("creator", liquidity)))
        .unwrap();

    assert_eq!(
        transmuter
            .spot_price(
                (deps.as_ref(), mock_env()),
                "denom0".to_string(),
                "denom0".to_string(),
            )
            .unwrap_err(),
        ContractError::SpotPriceQueryFailed {
            reason: "quote_asset_denom and base_asset_denom cannot be the same".to_string()
        }
    );

    assert_eq!(
        transmuter
            .spot_price(
                (deps.as_ref(), mock_env()),
                "random_denom".to_string(),
                "denom0".to_string(),
            )
            .unwrap_err(),
        ContractError::SpotPriceQueryFailed {
            reason: "quote_asset_denom is not in swappable assets: must be one of [\"denom0\", \"denom1\", \"factory/contract_address/transmuter/poolshare\"] but got random_denom".to_string()
        }
    );

    assert_eq!(
        transmuter
            .spot_price(
                (deps.as_ref(), mock_env()),
                "denom1".to_string(),
                "random_denom".to_string(),
            )
            .unwrap_err(),
        ContractError::SpotPriceQueryFailed {
            reason: "base_asset_denom is not in swappable assets: must be one of [\"denom0\", \"denom1\", \"factory/contract_address/transmuter/poolshare\"] but got random_denom".to_string()
        }
    );

    assert_eq!(
        transmuter
            .spot_price(
                (deps.as_ref(), mock_env()),
                "denom0".to_string(),
                "denom1".to_string(),
            )
            .unwrap()
            .spot_price,
        Decimal::one()
    );
}
