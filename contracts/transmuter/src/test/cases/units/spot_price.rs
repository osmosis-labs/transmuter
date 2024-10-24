use cosmwasm_std::{
    coin,
    testing::{message_info, mock_dependencies, mock_env},
    Coin, Decimal, Uint128,
};
use sylvia::types::{ExecCtx, InstantiateCtx, QueryCtx};

use crate::{asset::AssetConfig, contract::Transmuter, ContractError};

#[test]
fn test_spot_price_on_balanced_liquidity_must_be_one() {
    test_spot_price(&[coin(100_000, "denom0"), coin(100_000, "denom1")])
}
#[test]
fn test_spot_price_on_unbalanced_liquidity_must_be_one() {
    test_spot_price(&[coin(999_999_999, "denom0"), coin(100_000, "denom1")])
}

fn test_spot_price(liquidity: &[Coin]) {
    let transmuter = Transmuter::new();
    let mut deps = mock_dependencies();
    deps.api = deps.api.with_prefix("osmo");

    // make denom has non-zero total supply
    deps.querier
        .bank
        .update_balance("someone", vec![coin(1, "denom0"), coin(1, "denom1")]);

    let creator = deps.api.addr_make("creator");

    transmuter
        .instantiate(
            InstantiateCtx {
                deps: deps.as_mut(),
                env: mock_env(),
                info: message_info(&creator, &[]),
            },
            vec![
                AssetConfig::from_denom_str("denom0"),
                AssetConfig::from_denom_str("denom1"),
            ],
            "all".to_string(),
            Uint128::one(),
            None,
            "osmo1cyyzpxplxdzkeea7kwsydadg87357qnahakaks".to_string(),
        )
        .unwrap();

    transmuter
        .alloyed_asset
        .set_alloyed_denom(
            &mut deps.storage,
            &"factory/contract_address/all".to_string(),
        )
        .unwrap();

    let creator = deps.api.addr_make("creator");
    transmuter
        .join_pool(ExecCtx {
            deps: deps.as_mut(),
            env: mock_env(),
            info: message_info(&creator, liquidity),
        })
        .unwrap();

    assert_eq!(
        transmuter
            .spot_price(
                QueryCtx {
                    deps: deps.as_ref(),
                    env: mock_env(),
                },
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
                QueryCtx {
                    deps: deps.as_ref(),
                    env: mock_env(),
                },
                "random_denom".to_string(),
                "denom0".to_string(),
            )
            .unwrap_err(),
        ContractError::SpotPriceQueryFailed {
            reason: "base_asset_denom is not in swappable assets: must be one of [\"denom0\", \"denom1\", \"factory/contract_address/all\"] but got random_denom".to_string()
        }
    );

    assert_eq!(
        transmuter
            .spot_price(
                QueryCtx {
                    deps: deps.as_ref(),
                    env: mock_env(),
                },
                "denom1".to_string(),
                "random_denom".to_string(),
            )
            .unwrap_err(),
        ContractError::SpotPriceQueryFailed {
            reason: "quote_asset_denom is not in swappable assets: must be one of [\"denom0\", \"denom1\", \"factory/contract_address/all\"] but got random_denom".to_string()
        }
    );

    assert_eq!(
        transmuter
            .spot_price(
                QueryCtx {
                    deps: deps.as_ref(),
                    env: mock_env(),
                },
                "denom0".to_string(),
                "denom1".to_string(),
            )
            .unwrap()
            .spot_price,
        Decimal::one()
    );
}
