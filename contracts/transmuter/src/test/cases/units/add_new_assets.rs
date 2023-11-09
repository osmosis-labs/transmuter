use cosmwasm_std::Coin;
use osmosis_test_tube::OsmosisTestApp;

use crate::{
    asset::AssetConfig,
    contract::{GetShareDenomResponse, GetTotalPoolLiquidityResponse, InstantiateMsg},
    test::test_env::{assert_contract_err, TestEnvBuilder},
    ContractError,
};

#[test]
fn test_add_new_assets() {
    let app = OsmosisTestApp::new();

    // create denom
    app.init_account(&[
        Coin::new(1, "denom1"),
        Coin::new(1, "denom2"),
        Coin::new(1, "denom3"),
        Coin::new(1, "denom4"),
    ])
    .unwrap();

    let t = TestEnvBuilder::new()
        .with_account("admin", vec![])
        .with_account("non_admin", vec![])
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("denom1"),
                AssetConfig::from_denom_str("denom2"),
            ],
            admin: None, // override by admin account set above
            alloyed_asset_subdenom: "denomx".to_string(),
            moderator: None,
        })
        .build(&app);

    // add new asset
    let denoms = vec!["denom3".to_string(), "denom4".to_string()];

    let err = t
        .contract
        .execute(
            &crate::contract::ExecMsg::AddNewAssets {
                denoms: denoms.clone(),
            },
            &[],
            &t.accounts["non_admin"],
        )
        .unwrap_err();

    assert_contract_err(ContractError::Unauthorized {}, err);

    t.contract
        .execute(
            &crate::contract::ExecMsg::AddNewAssets { denoms },
            &[],
            &t.accounts["admin"],
        )
        .unwrap();

    // Get total pool liquidity
    let GetTotalPoolLiquidityResponse {
        total_pool_liquidity,
    } = t
        .contract
        .query(&crate::contract::QueryMsg::GetTotalPoolLiquidity {})
        .unwrap();

    assert_eq!(
        total_pool_liquidity,
        vec![
            Coin::new(0, "denom1"),
            Coin::new(0, "denom2"),
            Coin::new(0, "denom3"),
            Coin::new(0, "denom4"),
        ]
    );

    // Get alloyed denom
    let GetShareDenomResponse {
        share_denom: alloyed_denom,
    } = t
        .contract
        .query(&crate::contract::QueryMsg::GetShareDenom {})
        .unwrap();

    // Attempt to add alloyed_denom as asset, should error
    let err = t
        .contract
        .execute(
            &crate::contract::ExecMsg::AddNewAssets {
                denoms: vec![alloyed_denom.clone()],
            },
            &[],
            &t.accounts["admin"],
        )
        .unwrap_err();

    assert_contract_err(ContractError::ShareDenomNotAllowedAsPoolAsset {}, err);
}
