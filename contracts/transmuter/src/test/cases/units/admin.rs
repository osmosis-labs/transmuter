use cosmwasm_std::Coin;

use osmosis_std::types::cosmos::bank::v1beta1::{
    DenomUnit, Metadata, QueryDenomMetadataRequest, QueryDenomMetadataResponse,
};
use osmosis_test_tube::{OsmosisTestApp, Runner};

use crate::{
    contract::{ExecMsg, GetShareDenomResponse, InstantiateMsg, QueryMsg},
    test::test_env::{assert_contract_err, TestEnvBuilder},
};

const AXL_ETH: &str = "ibc/AXLETH";
const WH_ETH: &str = "ibc/WHETH";

#[test]
fn test_admin_set_denom_metadata() {
    let app = OsmosisTestApp::new();

    let alloyed_asset_subdenom = "eth";
    let t = TestEnvBuilder::new()
        .with_account("alice", vec![Coin::new(1_500, AXL_ETH)])
        .with_account("bob", vec![Coin::new(1_500, WH_ETH)])
        .with_account(
            "admin",
            vec![Coin::new(100_000, AXL_ETH), Coin::new(100_000, WH_ETH)],
        )
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_denoms: vec![AXL_ETH.to_string(), WH_ETH.to_string()],
            alloyed_asset_subdenom: alloyed_asset_subdenom.to_string(),
            admin: None,
            moderator: None,
        })
        .with_admin("admin")
        .build(&app);

    // pool share denom
    let GetShareDenomResponse { share_denom } =
        t.contract.query(&QueryMsg::GetShareDenom {}).unwrap();

    assert_eq!(
        format!(
            "factory/{}/alloyed/{}",
            t.contract.contract_addr, alloyed_asset_subdenom
        ),
        share_denom
    );

    let metadata_to_set = Metadata {
        base: share_denom.clone(),
        description: "Canonical ETH".to_string(),
        denom_units: vec![
            DenomUnit {
                denom: share_denom.clone(),
                exponent: 0,
                aliases: vec!["ueth".to_string()],
            },
            DenomUnit {
                denom: "eth".to_string(),
                exponent: 6,
                aliases: vec![],
            },
        ],
        display: "eth".to_string(),
        name: "Canonical ETH".to_string(),
        symbol: "ETH".to_string(),
    };

    // set denom metadata by non admin should fail
    let err = t
        .contract
        .execute(
            &ExecMsg::SetAlloyedDenomMetadata {
                metadata: metadata_to_set.clone(),
            },
            &[],
            &t.accounts["alice"],
        )
        .unwrap_err();

    assert_contract_err(crate::ContractError::Unauthorized {}, err);

    // set denom metadata
    t.contract
        .execute(
            &ExecMsg::SetAlloyedDenomMetadata {
                metadata: metadata_to_set.clone(),
            },
            &[],
            &t.accounts["admin"],
        )
        .unwrap();

    // query denom metadata
    let QueryDenomMetadataResponse { metadata } = app
        .query(
            "/cosmos.bank.v1beta1.Query/DenomMetadata",
            &QueryDenomMetadataRequest { denom: share_denom },
        )
        .unwrap();

    assert_eq!(metadata.unwrap(), metadata_to_set);
}
