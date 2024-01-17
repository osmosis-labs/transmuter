use std::{iter, path::PathBuf};

use crate::{
    asset::AssetConfig,
    contract::{ListAssetConfigsResponse, QueryMsg},
    test::{modules::cosmwasm_pool::CosmwasmPool, test_env::TransmuterContract},
};
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{to_binary, Coin, Uint128};
use osmosis_std::types::{
    cosmwasm::wasm::v1::{QueryAllContractStateRequest, QueryAllContractStateResponse},
    osmosis::cosmwasmpool::v1beta1::{
        ContractInfoByPoolIdRequest, ContractInfoByPoolIdResponse, MigratePoolContractsProposal,
        MsgCreateCosmWasmPool, UploadCosmWasmPoolCodeAndWhiteListProposal,
    },
};
use osmosis_test_tube::{Account, GovWithAppAccess, Module, OsmosisTestApp, Runner};

#[cw_serde]
struct InstantiateMsgV2 {
    pool_asset_denoms: Vec<String>,
    alloyed_asset_subdenom: String,
    admin: Option<String>,
    moderator: Option<String>,
}

#[test]
fn test_migrate_v2_to_v2_1() {
    // --- setup account ---
    let app = OsmosisTestApp::new();
    let signer = app
        .init_account(&[
            Coin::new(100000, "denom1"),
            Coin::new(100000, "denom2"),
            Coin::new(10000000000000, "uosmo"),
        ])
        .unwrap();

    // --- create pool ----

    let cp = CosmwasmPool::new(&app);
    let gov = GovWithAppAccess::new(&app);
    gov.propose_and_execute(
        UploadCosmWasmPoolCodeAndWhiteListProposal::TYPE_URL.to_string(),
        UploadCosmWasmPoolCodeAndWhiteListProposal {
            title: String::from("store test cosmwasm pool code"),
            description: String::from("test"),
            wasm_byte_code: get_prev_version_of_wasm_byte_code("v2"),
        },
        signer.address(),
        &signer,
    )
    .unwrap();

    let instantiate_msg = InstantiateMsgV2 {
        pool_asset_denoms: vec!["denom1".to_string(), "denom2".to_string()],
        alloyed_asset_subdenom: "denomx".to_string(),
        admin: Some(signer.address()),
        moderator: None,
    };

    let code_id = 1;
    let res = cp
        .create_cosmwasm_pool(
            MsgCreateCosmWasmPool {
                code_id,
                instantiate_msg: to_binary(&instantiate_msg).unwrap().to_vec(),
                sender: signer.address(),
            },
            &signer,
        )
        .unwrap();

    let pool_id = res.data.pool_id;

    let ContractInfoByPoolIdResponse {
        contract_address,
        code_id: _,
    } = cp
        .contract_info_by_pool_id(&ContractInfoByPoolIdRequest { pool_id })
        .unwrap();

    let t = TransmuterContract::new(&app, code_id, pool_id, contract_address.clone());

    // --- migrate pool ---
    let migrate_msg = crate::migrations::v2_1_0::MigrateMsg {
        asset_configs: vec![
            AssetConfig {
                denom: "denom1".to_string(),
                normalization_factor: Uint128::new(1),
            },
            AssetConfig {
                denom: "denom2".to_string(),
                normalization_factor: Uint128::new(10000),
            },
        ],
        alloyed_asset_normalization_factor: Uint128::new(10),
    };

    gov.propose_and_execute(
        MigratePoolContractsProposal::TYPE_URL.to_string(),
        MigratePoolContractsProposal {
            title: "migrate cosmwasmpool".to_string(),
            description: "test migration".to_string(),
            pool_ids: vec![pool_id],
            new_code_id: 0, // upload new code, so set this to 0
            wasm_byte_code: TransmuterContract::get_wasm_byte_code(),
            migrate_msg: to_binary(&migrate_msg).unwrap().to_vec(),
        },
        signer.address(),
        &signer,
    )
    .unwrap();

    let alloyed_denom = format!("factory/{contract_address}/alloyed/denomx");

    let expected_asset_configs = migrate_msg
        .asset_configs
        .into_iter()
        .chain(iter::once(AssetConfig {
            denom: alloyed_denom,
            normalization_factor: migrate_msg.alloyed_asset_normalization_factor,
        }))
        .collect::<Vec<_>>();

    // list asset configs
    let ListAssetConfigsResponse { asset_configs } =
        t.query(&QueryMsg::ListAssetConfigs {}).unwrap();

    assert_eq!(asset_configs, expected_asset_configs);
}

fn get_prev_version_of_wasm_byte_code(version: &str) -> Vec<u8> {
    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::fs::read(
        manifest_path
            .join("testdata")
            .join(format!("transmuter_{version}.wasm")),
    )
    .unwrap()
}
