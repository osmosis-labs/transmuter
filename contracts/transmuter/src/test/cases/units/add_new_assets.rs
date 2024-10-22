use cosmwasm_std::{coin, Uint128};
use osmosis_test_tube::OsmosisTestApp;

use crate::{
    asset::AssetConfig,
    contract::sv::InstantiateMsg,
    contract::{GetShareDenomResponse, GetTotalPoolLiquidityResponse},
    test::test_env::{assert_contract_err, TestEnvBuilder},
    ContractError,
};

#[test]
fn test_add_new_assets() {
    let app = OsmosisTestApp::new();

    // create denom
    app.init_account(&[
        coin(1, "denom1"),
        coin(1, "denom2"),
        coin(1, "denom3"),
        coin(1, "denom4"),
    ])
    .unwrap();

    let t = TestEnvBuilder::new()
        .with_account("admin", vec![])
        .with_account("non_admin", vec![])
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_configs: vec![AssetConfig::from_denom_str("denom1")],
            admin: None, // override by admin account set above
            alloyed_asset_subdenom: "denomx".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
            moderator: "osmo1cyyzpxplxdzkeea7kwsydadg87357qnahakaks".to_string(),
        })
        .build(&app);

    // add new asset
    let denoms = [
        "denom2".to_string(),
        "denom3".to_string(),
        "denom4".to_string(),
    ];

    let err = t
        .contract
        .execute(
            &crate::contract::sv::ExecMsg::AddNewAssets {
                asset_configs: denoms
                    .iter()
                    .map(|denom| AssetConfig::from_denom_str(denom.as_str()))
                    .collect(),
            },
            &[],
            &t.accounts["non_admin"],
        )
        .unwrap_err();

    assert_contract_err(ContractError::Unauthorized {}, err);

    t.contract
        .execute(
            &crate::contract::sv::ExecMsg::AddNewAssets {
                asset_configs: denoms
                    .iter()
                    .map(|denom| AssetConfig::from_denom_str(denom.as_str()))
                    .collect(),
            },
            &[],
            &t.accounts["admin"],
        )
        .unwrap();

    // Get total pool liquidity
    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .contract
        .query(&crate::contract::sv::QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![
            coin(0, "denom1"),
            coin(0, "denom2"),
            coin(0, "denom3"),
            coin(0, "denom4"),
        ]
    );

    // Get alloyed denom
    let GetShareDenomResponse {
        share_denom: alloyed_denom,
    } = t
        .contract
        .query(&crate::contract::sv::QueryMsg::GetShareDenom {})
        .unwrap();

    // Attempt to add alloyed_denom as asset, should error
    let err = t
        .contract
        .execute(
            &crate::contract::sv::ExecMsg::AddNewAssets {
                asset_configs: vec![AssetConfig::from_denom_str(alloyed_denom.as_str())],
            },
            &[],
            &t.accounts["admin"],
        )
        .unwrap_err();

    assert_contract_err(ContractError::ShareDenomNotAllowedAsPoolAsset {}, err);
}
