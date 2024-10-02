// use std::{iter, path::PathBuf};

// use crate::{
//     asset::AssetConfig,
//     contract::{
//         sv::{InstantiateMsg, QueryMsg},
//         GetModeratorResponse, ListAssetConfigsResponse,
//     },
//     migrations::v3_2_0::MigrateMsg,
//     test::{modules::cosmwasm_pool::CosmwasmPool, test_env::TransmuterContract},
// };
// use cosmwasm_schema::cw_serde;
// use cosmwasm_std::{from_json, to_json_binary, Coin, Uint128};
// use osmosis_std::types::{
//     cosmwasm::wasm::v1::{QueryRawContractStateRequest, QueryRawContractStateResponse},
//     osmosis::cosmwasmpool::v1beta1::{
//         ContractInfoByPoolIdRequest, ContractInfoByPoolIdResponse, MigratePoolContractsProposal,
//         MsgCreateCosmWasmPool, UploadCosmWasmPoolCodeAndWhiteListProposal,
//     },
// };
// use osmosis_test_tube::{Account, GovWithAppAccess, Module, OsmosisTestApp, Runner};
// use rstest::rstest;

// #[cw_serde]
// struct InstantiateMsgV2 {
//     pool_asset_denoms: Vec<String>,
//     alloyed_asset_subdenom: String,
//     admin: Option<String>,
//     moderator: Option<String>,
// }

// #[cw_serde]
// struct MigrateMsgV3 {
//     asset_configs: Vec<AssetConfig>,
//     alloyed_asset_normalization_factor: Uint128,
//     moderator: Option<String>,
// }

// #[test]
// fn test_migrate_v2_to_v3() {
//     // --- setup account ---
//     let app = OsmosisTestApp::new();
//     let signer = app
//         .init_account(&[
//             Coin::new(100000, "denom1"),
//             Coin::new(100000, "denom2"),
//             Coin::new(10000000000000, "uosmo"),
//         ])
//         .unwrap();

//     // --- create pool ----

//     let cp = CosmwasmPool::new(&app);
//     let gov = GovWithAppAccess::new(&app);
//     gov.propose_and_execute(
//         UploadCosmWasmPoolCodeAndWhiteListProposal::TYPE_URL.to_string(),
//         UploadCosmWasmPoolCodeAndWhiteListProposal {
//             title: String::from("store test cosmwasm pool code"),
//             description: String::from("test"),
//             wasm_byte_code: get_prev_version_of_wasm_byte_code("v2"),
//         },
//         signer.address(),
//         &signer,
//     )
//     .unwrap();

//     let instantiate_msg = InstantiateMsgV2 {
//         pool_asset_denoms: vec!["denom1".to_string(), "denom2".to_string()],
//         alloyed_asset_subdenom: "denomx".to_string(),
//         admin: Some(signer.address()),
//         moderator: None,
//     };

//     let code_id = 1;
//     let res = cp
//         .create_cosmwasm_pool(
//             MsgCreateCosmWasmPool {
//                 code_id,
//                 instantiate_msg: to_json_binary(&instantiate_msg).unwrap().to_vec(),
//                 sender: signer.address(),
//             },
//             &signer,
//         )
//         .unwrap();

//     let pool_id = res.data.pool_id;

//     let ContractInfoByPoolIdResponse {
//         contract_address,
//         code_id: _,
//     } = cp
//         .contract_info_by_pool_id(&ContractInfoByPoolIdRequest { pool_id })
//         .unwrap();

//     let t = TransmuterContract::new(&app, code_id, pool_id, contract_address.clone());

//     // --- migrate pool ---
//     let migrate_msg = MigrateMsgV3 {
//         asset_configs: vec![
//             AssetConfig {
//                 denom: "denom1".to_string(),
//                 normalization_factor: Uint128::new(1),
//             },
//             AssetConfig {
//                 denom: "denom2".to_string(),
//                 normalization_factor: Uint128::new(10000),
//             },
//         ],
//         alloyed_asset_normalization_factor: Uint128::new(10),
//         moderator: Some("osmo1cyyzpxplxdzkeea7kwsydadg87357qnahakaks".to_string()),
//     };

//     gov.propose_and_execute(
//         MigratePoolContractsProposal::TYPE_URL.to_string(),
//         MigratePoolContractsProposal {
//             title: "migrate cosmwasmpool".to_string(),
//             description: "test migration".to_string(),
//             pool_ids: vec![pool_id],
//             new_code_id: 0, // upload new code, so set this to 0
//             wasm_byte_code: get_prev_version_of_wasm_byte_code("v3"),
//             migrate_msg: to_json_binary(&migrate_msg).unwrap().to_vec(),
//         },
//         signer.address(),
//         &signer,
//     )
//     .unwrap();

//     let alloyed_denom = format!("factory/{contract_address}/alloyed/denomx");

//     let expected_asset_configs = migrate_msg
//         .asset_configs
//         .into_iter()
//         .chain(iter::once(AssetConfig {
//             denom: alloyed_denom,
//             normalization_factor: migrate_msg.alloyed_asset_normalization_factor,
//         }))
//         .collect::<Vec<_>>();

//     // list asset configs
//     let ListAssetConfigsResponse { asset_configs } =
//         t.query(&QueryMsg::ListAssetConfigs {}).unwrap();

//     assert_eq!(asset_configs, expected_asset_configs);

//     let GetModeratorResponse { moderator } = t.query(&QueryMsg::GetModerator {}).unwrap();
//     assert_eq!(moderator, migrate_msg.moderator.unwrap());
// }

// #[rstest]
// #[case("v3")]
// #[case("v3_1")]
// fn test_migrate_v3(#[case] from_version: &str) {
//     // --- setup account ---
//     let app = OsmosisTestApp::new();
//     let signer = app
//         .init_account(&[
//             Coin::new(100000, "denom1"),
//             Coin::new(100000, "denom2"),
//             Coin::new(10000000000000, "uosmo"),
//         ])
//         .unwrap();

//     // --- create pool ----

//     let cp = CosmwasmPool::new(&app);
//     let gov = GovWithAppAccess::new(&app);
//     gov.propose_and_execute(
//         UploadCosmWasmPoolCodeAndWhiteListProposal::TYPE_URL.to_string(),
//         UploadCosmWasmPoolCodeAndWhiteListProposal {
//             title: String::from("store test cosmwasm pool code"),
//             description: String::from("test"),
//             wasm_byte_code: get_prev_version_of_wasm_byte_code(from_version),
//         },
//         signer.address(),
//         &signer,
//     )
//     .unwrap();

//     let instantiate_msg = InstantiateMsg {
//         pool_asset_configs: vec![
//             AssetConfig {
//                 denom: "denom1".to_string(),
//                 normalization_factor: Uint128::new(1),
//             },
//             AssetConfig {
//                 denom: "denom2".to_string(),
//                 normalization_factor: Uint128::new(10000),
//             },
//         ],
//         alloyed_asset_subdenom: "denomx".to_string(),
//         alloyed_asset_normalization_factor: Uint128::new(10),
//         admin: Some(signer.address()),
//         moderator: signer.address(),
//     };

//     let code_id = 1;
//     let res = cp
//         .create_cosmwasm_pool(
//             MsgCreateCosmWasmPool {
//                 code_id,
//                 instantiate_msg: to_json_binary(&instantiate_msg).unwrap().to_vec(),
//                 sender: signer.address(),
//             },
//             &signer,
//         )
//         .unwrap();

//     let pool_id = res.data.pool_id;

//     let ContractInfoByPoolIdResponse {
//         contract_address,
//         code_id: _,
//     } = cp
//         .contract_info_by_pool_id(&ContractInfoByPoolIdRequest { pool_id })
//         .unwrap();

//     let t = TransmuterContract::new(&app, code_id, pool_id, contract_address.clone());

//     // --- migrate pool ---
//     let migrate_msg = MigrateMsg {};

//     gov.propose_and_execute(
//         MigratePoolContractsProposal::TYPE_URL.to_string(),
//         MigratePoolContractsProposal {
//             title: "migrate cosmwasmpool".to_string(),
//             description: "test migration".to_string(),
//             pool_ids: vec![pool_id],
//             new_code_id: 0, // upload new code, so set this to 0
//             wasm_byte_code: TransmuterContract::get_wasm_byte_code(),
//             migrate_msg: to_json_binary(&migrate_msg).unwrap().to_vec(),
//         },
//         signer.address(),
//         &signer,
//     )
//     .unwrap();

//     let alloyed_denom = format!("factory/{contract_address}/alloyed/denomx");

//     let expected_asset_configs = instantiate_msg
//         .pool_asset_configs
//         .into_iter()
//         .chain(iter::once(AssetConfig {
//             denom: alloyed_denom,
//             normalization_factor: instantiate_msg.alloyed_asset_normalization_factor,
//         }))
//         .collect::<Vec<_>>();

//     // list asset configs
//     let ListAssetConfigsResponse { asset_configs } =
//         t.query(&QueryMsg::ListAssetConfigs {}).unwrap();

//     // expect no changes in asset config
//     assert_eq!(asset_configs, expected_asset_configs);

//     let res: QueryRawContractStateResponse = app
//         .query(
//             "/cosmwasm.wasm.v1.Query/RawContractState",
//             &QueryRawContractStateRequest {
//                 address: t.contract_addr,
//                 query_data: b"contract_info".to_vec(),
//             },
//         )
//         .unwrap();

//     let version: cw2::ContractVersion = from_json(res.data).unwrap();

//     assert_eq!(
//         version,
//         cw2::ContractVersion {
//             contract: "crates.io:transmuter".to_string(),
//             version: "3.2.0".to_string()
//         }
//     );
// }

// fn get_prev_version_of_wasm_byte_code(version: &str) -> Vec<u8> {
//     let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
//     let wasm_path = manifest_path
//         .join("testdata")
//         .join(format!("transmuter_{version}.wasm"));

//     let err_msg = &format!("failed to read wasm file: {}", wasm_path.display());
//     std::fs::read(wasm_path).expect(err_msg)
// }
