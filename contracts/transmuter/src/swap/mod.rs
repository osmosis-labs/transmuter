mod common;

mod alloyed_asset_to_tokens;
mod non_alloyed_exact_amount_in;
mod non_alloyed_exact_amount_out;
mod tokens_to_alloyed_asset;

pub use common::*;

pub use alloyed_asset_to_tokens::{BurnTarget, SwapFromAlloyedConstraint};
pub use tokens_to_alloyed_asset::SwapToAlloyedConstraint;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::scope::Scope;
    use crate::swap::common::construct_scope_value_pairs;
    use crate::{
        asset::Asset,
        corruptable::Corruptable,
        swap::common::{Entrypoint, SwapExactAmountInResponseData, SwapExactAmountOutResponseData},
        transmuter_pool::AssetGroup,
        ContractError,
    };
    use crate::{contract::Transmuter, transmuter_pool::TransmuterPool};
    use cosmwasm_std::Decimal;
    use cosmwasm_std::{
        coin, from_json,
        testing::{
            mock_dependencies, mock_env, MockApi, MockQuerier, MockStorage, MOCK_CONTRACT_ADDR,
        },
        to_json_binary, Addr, BankMsg, Coin, Coins, CosmosMsg, OwnedDeps, Response, SubMsg,
        Uint128,
    };
    use itertools::Itertools;
    use osmosis_std::types::osmosis::tokenfactory::v1beta1::{MsgBurn, MsgMint};
    use osmosis_test_tube::cosmrs::proto::prost::Message;
    use rstest::rstest;
    use transmuter_math::rebalancing::config::RebalancingConfig;

    #[rstest]
    #[case(
        Entrypoint::Exec,
        SwapToAlloyedConstraint::ExactIn {
            tokens_in: &[coin(100, "denom1")],
            token_out_min_amount: Uint128::one(),
        },
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgMint {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(10000u128, "alloyed").into()),
                mint_to_address: "addr1".to_string()
            })),
    )]
    #[case(
        Entrypoint::Sudo,
        SwapToAlloyedConstraint::ExactIn {
            tokens_in: &[coin(100, "denom1")],
            token_out_min_amount: Uint128::one(),
        },
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .set_data(to_json_binary(&SwapExactAmountInResponseData {
                token_out_amount: Uint128::new(10000u128)
            }).unwrap())
            .add_message(MsgMint {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(10000u128, "alloyed").into()),
                mint_to_address: "addr1".to_string()
            })),
    )]
    #[case(
        Entrypoint::Exec,
        SwapToAlloyedConstraint::ExactOut {
            token_in_denom: "denom1",
            token_in_max_amount: Uint128::new(100),
            token_out_amount: Uint128::new(10000u128)
        },
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgMint {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(10000u128, "alloyed").into()),
                mint_to_address: "addr1".to_string()
            })),
    )]
    #[case(
        Entrypoint::Sudo,
        SwapToAlloyedConstraint::ExactOut {
            token_in_denom: "denom1",
            token_in_max_amount: Uint128::new(100),
            token_out_amount: Uint128::new(10000u128)
        },
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .set_data(to_json_binary(&SwapExactAmountOutResponseData {
                token_in_amount: Uint128::new(100u128)
            }).unwrap())
            .add_message(MsgMint {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(10000u128, "alloyed").into()),
                mint_to_address: "addr1".to_string()
            })),
    )]
    fn test_swap_tokens_to_alloyed_asset(
        #[case] entrypoint: Entrypoint,
        #[case] constraint: SwapToAlloyedConstraint,
        #[case] mint_to_address: Addr,
        #[case] expected_res: Result<Response, ContractError>,
    ) {
        let mut deps = mock_dependencies();
        let transmuter = Transmuter::new();
        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, 100u128.into())
            .unwrap();

        transmuter
            .pool
            .save(
                &mut deps.storage,
                &TransmuterPool {
                    pool_assets: vec![
                        Asset::new(Uint128::from(1000u128), "denom1", 1u128).unwrap(),
                        Asset::new(Uint128::from(1000u128), "denom2", 10u128).unwrap(),
                    ],
                    asset_groups: BTreeMap::new(),
                },
            )
            .unwrap();

        let res = transmuter.swap_tokens_to_alloyed_asset(
            entrypoint,
            constraint,
            mint_to_address,
            deps.as_mut(),
            mock_env(),
        );

        assert_eq!(res, expected_res);
    }

    #[rstest]
    #[case(
        Entrypoint::Exec,
        SwapFromAlloyedConstraint::ExactIn {
            token_out_denom: "denom1",
            token_out_min_amount: Uint128::from(1u128),
            token_in_amount: Uint128::from(100u128),
        },
        BurnTarget::SenderAccount,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(100u128, "alloyed").into()),
                burn_from_address: "addr1".to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1u128, "denom1")]
            }))
    )]
    #[case(
        Entrypoint::Sudo,
        SwapFromAlloyedConstraint::ExactIn {
            token_out_denom: "denom1",
            token_out_min_amount: Uint128::from(1u128),
            token_in_amount: Uint128::from(100u128),
        },
        BurnTarget::SentFunds,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(100u128, "alloyed").into()),
                burn_from_address: MOCK_CONTRACT_ADDR.to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1u128, "denom1")]
            })
            .set_data(to_json_binary(&SwapExactAmountInResponseData {
                token_out_amount: Uint128::from(1u128)
            }).unwrap()))
    )]
    #[case(
        Entrypoint::Exec,
        SwapFromAlloyedConstraint::ExactOut {
            tokens_out: &[coin(1u128, "denom1")],
            token_in_max_amount: Uint128::from(100u128),
        },
        BurnTarget::SenderAccount,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(100u128, "alloyed").into()),
                burn_from_address: "addr1".to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1u128, "denom1")]
            }))
    )]
    #[case(
        Entrypoint::Sudo,
        SwapFromAlloyedConstraint::ExactOut {
            tokens_out: &[coin(1u128, "denom1")],
            token_in_max_amount: Uint128::from(100u128),
        },
        BurnTarget::SentFunds,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(100u128, "alloyed").into()),
                burn_from_address: MOCK_CONTRACT_ADDR.to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1u128, "denom1")]
            })
            .set_data(to_json_binary(&SwapExactAmountOutResponseData {
                token_in_amount: Uint128::from(100u128)
            }).unwrap()))
    )]
    fn test_swap_alloyed_asset_to_tokens(
        #[case] entrypoint: Entrypoint,
        #[case] constraint: SwapFromAlloyedConstraint,
        #[case] burn_target: BurnTarget,
        #[case] sender: Addr,
        #[case] expected_res: Result<Response, ContractError>,
    ) {
        let alloyed_holder = match burn_target {
            BurnTarget::SenderAccount => sender.to_string(),
            BurnTarget::SentFunds => MOCK_CONTRACT_ADDR.to_string(),
        };

        let mut deps = cosmwasm_std::testing::mock_dependencies_with_balances(&[(
            alloyed_holder.as_str(),
            &[coin(110000000000000u128, "alloyed")],
        )]);

        let transmuter = Transmuter::new();
        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, 100u128.into())
            .unwrap();

        transmuter
            .pool
            .save(
                &mut deps.storage,
                &TransmuterPool {
                    pool_assets: vec![
                        Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                        Asset::new(Uint128::from(1000000000000u128), "denom2", 10u128).unwrap(),
                    ],
                    asset_groups: BTreeMap::new(),
                },
            )
            .unwrap();

        let res = transmuter.swap_alloyed_asset_to_tokens(
            entrypoint,
            constraint,
            burn_target,
            sender,
            deps.as_mut(),
            mock_env(),
        );

        assert_eq!(res, expected_res);

        let pool = transmuter.pool.load(&deps.storage).unwrap();

        for denom in ["denom1", "denom2"] {
            assert!(pool.has_denom(denom))
        }
    }

    #[rstest]
    #[case(
        Entrypoint::Sudo,
        SwapFromAlloyedConstraint::ExactOut {
            tokens_out: &[coin(1000000000000u128, "denom1")],
            token_in_max_amount: Uint128::from(100000000000000u128),
        },
        vec!["denom1"],
        vec!["denom1"],
        BurnTarget::SentFunds,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(100000000000000u128, "alloyed").into()),
                burn_from_address: MOCK_CONTRACT_ADDR.to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1000000000000u128, "denom1")]
            })
            .set_data(to_json_binary(&SwapExactAmountOutResponseData {
                token_in_amount: Uint128::from(100000000000000u128)
            }).unwrap()))
    )]
    #[case(
        Entrypoint::Sudo,
        SwapFromAlloyedConstraint::ExactIn {
            token_out_denom: "denom1",
            token_out_min_amount: 1000000000000u128.into(),
            token_in_amount: 100000000000000u128.into(),
        },
        vec!["denom1"],
        vec!["denom1"],
        BurnTarget::SentFunds,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(100000000000000u128, "alloyed").into()),
                burn_from_address: MOCK_CONTRACT_ADDR.to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1000000000000u128, "denom1")]
            })
            .set_data(to_json_binary(&SwapExactAmountInResponseData {
                token_out_amount: 1000000000000u128.into(),
            }).unwrap()))
    )]
    #[case(
        Entrypoint::Exec,
        SwapFromAlloyedConstraint::ExactIn {
            token_out_denom: "denom1",
            token_out_min_amount: 1000000000000u128.into(),
            token_in_amount: 100000000000000u128.into(),
        },
        vec!["denom1"],
        vec!["denom1"],
        BurnTarget::SenderAccount,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(100000000000000u128, "alloyed").into()),
                burn_from_address: "addr1".to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1000000000000u128, "denom1")]
            }))
    )]
    #[case(
        Entrypoint::Exec,
        SwapFromAlloyedConstraint::ExactOut {
            tokens_out: &[coin(1000000000000u128, "denom1")],
            token_in_max_amount: Uint128::from(100000000000000u128),
        },
        vec!["denom1"],
        vec!["denom1"],
        BurnTarget::SenderAccount,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(100000000000000u128, "alloyed").into()),
                burn_from_address: "addr1".to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1000000000000u128, "denom1")]
            }))
    )]
    #[case(
        Entrypoint::Sudo,
        SwapFromAlloyedConstraint::ExactOut {
            tokens_out: &[coin(1000000000000u128, "denom1"), coin(1000000000000u128, "denom2")],
            token_in_max_amount: Uint128::from(110000000000000u128),
        },
        vec!["denom1", "denom2"],
        vec!["denom1", "denom2"],
        BurnTarget::SentFunds,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(110000000000000u128, "alloyed").into()),
                burn_from_address: MOCK_CONTRACT_ADDR.to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1000000000000u128, "denom1"), coin(1000000000000u128, "denom2")]
            })
            .set_data(to_json_binary(&SwapExactAmountOutResponseData {
                token_in_amount: Uint128::from(110000000000000u128),
            }).unwrap()))
    )]
    #[case(
        Entrypoint::Sudo,
        SwapFromAlloyedConstraint::ExactOut {
            tokens_out: &[coin(1000000000000u128, "denom1"), coin(500000000000u128, "denom2")],
            token_in_max_amount: Uint128::from(105000000000000u128),
        },
        vec!["denom1", "denom2"],
        vec!["denom1"],
        BurnTarget::SentFunds,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(MsgBurn {
                sender: MOCK_CONTRACT_ADDR.to_string(),
                amount: Some(coin(105000000000000u128, "alloyed").into()),
                burn_from_address: MOCK_CONTRACT_ADDR.to_string()
            })
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1000000000000u128, "denom1"), coin(500000000000u128, "denom2")],
            })
            .set_data(to_json_binary(&SwapExactAmountOutResponseData {
                token_in_amount: Uint128::from(105000000000000u128),
            }).unwrap()))
    )]
    fn test_swap_alloyed_asset_to_tokens_with_corrupted_assets(
        #[case] entrypoint: Entrypoint,
        #[case] constraint: SwapFromAlloyedConstraint,
        #[case] corrupted_denoms: Vec<&str>,
        #[case] removed_denoms: Vec<&str>,
        #[case] burn_target: BurnTarget,
        #[case] sender: Addr,
        #[case] expected_res: Result<Response, ContractError>,
    ) {
        let alloyed_holder = match burn_target {
            BurnTarget::SenderAccount => sender.to_string(),
            BurnTarget::SentFunds => MOCK_CONTRACT_ADDR.to_string(),
        };

        let mut deps = cosmwasm_std::testing::mock_dependencies_with_balances(&[(
            alloyed_holder.as_str(),
            &[coin(210000000000000u128, "alloyed")],
        )]);

        let transmuter = Transmuter::new();
        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, 100u128.into())
            .unwrap();

        let mut pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(), // 1000000000000 * 100
                Asset::new(Uint128::from(1000000000000u128), "denom2", 10u128).unwrap(), // 1000000000000 * 10
                Asset::new(Uint128::from(1000000000000u128), "denom3", 1u128).unwrap(), // 1000000000000 * 100
            ],
            asset_groups: BTreeMap::new(),
        };

        let all_denoms = pool
            .clone()
            .pool_assets
            .into_iter()
            .map(|asset| asset.denom().to_string())
            .collect::<Vec<String>>();

        for denom in corrupted_denoms {
            pool.mark_corrupted_asset(denom).unwrap();
        }

        transmuter.pool.save(&mut deps.storage, &pool).unwrap();

        for denom in all_denoms.clone() {
            transmuter
                .rebalancer
                .add_config(
                    &mut deps.storage,
                    Scope::denom(denom.as_str()),
                    RebalancingConfig::limit_only(Decimal::percent(100)).unwrap(),
                )
                .unwrap();
        }

        let res = transmuter.swap_alloyed_asset_to_tokens(
            entrypoint,
            constraint,
            burn_target,
            sender,
            deps.as_mut(),
            mock_env(),
        );

        assert_eq!(res, expected_res);

        // all drained denoms that are corrupted should not be in the pool
        let pool = transmuter.pool.load(&deps.storage).unwrap();

        for denom in all_denoms {
            if removed_denoms.contains(&denom.as_str()) {
                assert!(
                    !pool.has_denom(denom.as_str()),
                    "must not contain {} since it's corrupted and drained",
                    denom
                );

                // limiters should be removed
                assert!(
                    transmuter
                        .rebalancer
                        .get_config_by_scope(&deps.storage, &Scope::denom(denom.as_str()))
                        .unwrap()
                        .is_none(),
                    "must not contain limiter for {} since it's corrupted and drained",
                    denom
                );
            } else {
                assert!(
                    pool.has_denom(denom.as_str()),
                    "must contain {} since it's not corrupted or not drained",
                    denom
                );

                // limiters should not be removed
                assert!(
                    transmuter
                        .rebalancer
                        .get_config_by_scope(&deps.storage, &Scope::denom(denom.as_str()))
                        .unwrap()
                        .is_some(),
                    "must contain limiter for {} since it's not corrupted or not drained",
                    denom
                );
            }
        }
    }

    #[test]
    fn test_swap_non_alloyed_exact_amount_in_with_corrupted_assets() {
        let mut deps = mock_dependencies();
        let transmuter = Transmuter::new();
        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, 100u128.into())
            .unwrap();

        let mut pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(), // 1000000000000 * 100
                Asset::new(Uint128::from(1000000000000u128), "denom2", 10u128).unwrap(), // 1000000000000 * 10
                Asset::new(Uint128::from(1000000000000u128), "denom3", 1u128).unwrap(), // 1000000000000 * 100
            ],
            asset_groups: BTreeMap::new(),
        };

        let all_denoms = pool
            .clone()
            .pool_assets
            .into_iter()
            .map(|asset| asset.denom().to_string())
            .collect::<Vec<_>>();

        pool.mark_corrupted_asset("denom1").unwrap();

        transmuter.pool.save(&mut deps.storage, &pool).unwrap();

        for denom in all_denoms.clone() {
            transmuter
                .rebalancer
                .add_config(
                    &mut deps.storage,
                    Scope::denom(denom.as_str()),
                    RebalancingConfig::limit_only(Decimal::percent(100)).unwrap(),
                )
                .unwrap();
        }

        transmuter
            .swap_non_alloyed_exact_amount_in(
                coin(1000000000000, "denom3"),
                "denom1",
                1000000000000u128.into(),
                deps.api.addr_make("sender"),
                deps.as_mut(),
            )
            .unwrap();

        // all drained denoms that are corrupted should not be in the pool
        let pool = transmuter.pool.load(&deps.storage).unwrap();

        let denoms = pool
            .pool_assets
            .into_iter()
            .map(|a| a.denom().to_string())
            .collect_vec();

        assert_eq!(denoms, vec!["denom2", "denom3"]);

        let limiter_denoms = transmuter
            .rebalancer
            .list_configs(&deps.storage)
            .unwrap()
            .into_iter()
            .map(|(denom, _)| denom)
            .unique()
            .collect_vec();

        assert_eq!(
            limiter_denoms,
            vec![Scope::denom("denom2").key(), Scope::denom("denom3").key()]
        );
    }

    #[test]
    fn test_swap_non_alloyed_exact_amount_out_with_corrupted_assets() {
        let mut deps = mock_dependencies();
        let transmuter = Transmuter::new();
        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, 100u128.into())
            .unwrap();

        let mut pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(), // 1000000000000 * 100
                Asset::new(Uint128::from(1000000000000u128), "denom2", 10u128).unwrap(), // 1000000000000 * 10
                Asset::new(Uint128::from(1000000000000u128), "denom3", 1u128).unwrap(), // 1000000000000 * 100
            ],
            asset_groups: BTreeMap::new(),
        };

        let all_denoms = pool
            .clone()
            .pool_assets
            .into_iter()
            .map(|asset| asset.denom().to_string())
            .collect::<Vec<_>>();

        pool.mark_corrupted_asset("denom1").unwrap();

        transmuter.pool.save(&mut deps.storage, &pool).unwrap();

        for denom in all_denoms.clone() {
            transmuter
                .rebalancer
                .add_config(
                    &mut deps.storage,
                    Scope::denom(denom.as_str()),
                    RebalancingConfig::limit_only(Decimal::percent(100)).unwrap(),
                )
                .unwrap();
        }

        transmuter
            .swap_non_alloyed_exact_amount_out(
                "denom3",
                1000000000000u128.into(),
                coin(1000000000000, "denom1"),
                deps.api.addr_make("sender"),
                deps.as_mut(),
            )
            .unwrap();

        // all drained denoms that are corrupted should not be in the pool
        let pool = transmuter.pool.load(&deps.storage).unwrap();

        let denoms = pool
            .pool_assets
            .into_iter()
            .map(|a| a.denom().to_string())
            .collect_vec();

        assert_eq!(denoms, vec!["denom2", "denom3"]);

        let limiter_denoms = transmuter
            .rebalancer
            .list_configs(&deps.storage)
            .unwrap()
            .into_iter()
            .map(|(denom, _)| denom)
            .unique()
            .collect_vec();

        assert_eq!(
            limiter_denoms,
            vec![Scope::denom("denom2").key(), Scope::denom("denom3").key()]
        );
    }

    #[rstest]
    #[case(
        coin(100u128, "denom1"),
        "denom2",
        1000u128,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1000u128, "denom2")]
            })
            .set_data(to_json_binary(&SwapExactAmountInResponseData {
                token_out_amount: Uint128::from(1000u128)
            }).unwrap()))
    )]
    #[case(
        coin(100u128, "denom2"),
        "denom1",
        10u128,
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(10u128, "denom1")]
            })
            .set_data(to_json_binary(&SwapExactAmountInResponseData {
                token_out_amount: Uint128::from(10u128)
            }).unwrap()))
    )]
    #[case(
        coin(100u128, "denom2"),
        "denom1",
        100u128,
        Addr::unchecked("addr1"),
        Err(ContractError::InsufficientTokenOut {
            min_required: 100u128.into(),
            amount_out: 10u128.into()
        })
    )]
    #[case(
        coin(100000000001u128, "denom1"),
        "denom2",
        1000000000010u128,
        Addr::unchecked("addr1"),
        Err(ContractError::InsufficientPoolAsset {
            required: coin(1000000000010u128, "denom2"),
            available: coin(1000000000000u128, "denom2"),
        })
    )]
    fn test_swap_non_alloyed_exact_amount_in(
        #[case] token_in: Coin,
        #[case] token_out_denom: &str,
        #[case] token_out_min_amount: u128,
        #[case] sender: Addr,
        #[case] expected_res: Result<Response, ContractError>,
    ) {
        let mut deps = cosmwasm_std::testing::mock_dependencies_with_balances(&[(
            sender.to_string().as_str(),
            &[coin(2000000000000u128, "alloyed")],
        )]);

        let transmuter = Transmuter::new();
        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, 100u128.into())
            .unwrap();

        transmuter
            .pool
            .save(
                &mut deps.storage,
                &TransmuterPool {
                    pool_assets: vec![
                        Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                        Asset::new(Uint128::from(1000000000000u128), "denom2", 10u128).unwrap(),
                    ],
                    asset_groups: BTreeMap::new(),
                },
            )
            .unwrap();

        let res = transmuter.swap_non_alloyed_exact_amount_in(
            token_in.clone(),
            token_out_denom,
            token_out_min_amount.into(),
            sender,
            deps.as_mut(),
        );

        assert_eq!(res, expected_res);
    }

    #[rstest]
    #[case(
        "denom1",
        100u128,
        coin(1000u128, "denom2"),
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(1000u128, "denom2")]
            })
            .set_data(to_json_binary(&SwapExactAmountOutResponseData {
                token_in_amount: 100u128.into()
            }).unwrap()))
    )]
    #[case(
        "denom2",
        100u128,
        coin(10u128, "denom1"),
        Addr::unchecked("addr1"),
        Ok(Response::new()
            .add_message(BankMsg::Send {
                to_address: "addr1".to_string(),
                amount: vec![coin(10u128, "denom1")]
            })
            .set_data(to_json_binary(&SwapExactAmountOutResponseData {
                token_in_amount: 100u128.into()
            }).unwrap()))
    )]
    #[case(
        "denom2",
        100u128,
        coin(100u128, "denom1"),
        Addr::unchecked("addr1"),
        Err(ContractError::ExcessiveRequiredTokenIn {
            limit: 100u128.into(),
            required: 1000u128.into()
        })
    )]
    #[case(
        "denom1",
        100000000001u128,
        coin(1000000000010u128, "denom2"),
        Addr::unchecked("addr1"),
        Err(ContractError::InsufficientPoolAsset {
            required: coin(1000000000010u128, "denom2"),
            available: coin(1000000000000u128, "denom2"),
        })
    )]
    fn test_swap_non_alloyed_exact_amount_out(
        #[case] token_in_denom: &str,
        #[case] token_in_max_amount: u128,
        #[case] token_out: Coin,
        #[case] sender: Addr,
        #[case] expected_res: Result<Response, ContractError>,
    ) {
        let mut deps = cosmwasm_std::testing::mock_dependencies_with_balances(&[(
            sender.to_string().as_str(),
            &[coin(2000000000000u128, "alloyed")],
        )]);

        let transmuter = Transmuter::new();
        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, 100u128.into())
            .unwrap();

        transmuter
            .pool
            .save(
                &mut deps.storage,
                &TransmuterPool {
                    pool_assets: vec![
                        Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                        Asset::new(Uint128::from(1000000000000u128), "denom2", 10u128).unwrap(),
                    ],
                    asset_groups: BTreeMap::new(),
                },
            )
            .unwrap();

        let res = transmuter.swap_non_alloyed_exact_amount_out(
            token_in_denom,
            token_in_max_amount.into(),
            token_out,
            sender,
            deps.as_mut(),
        );

        assert_eq!(res, expected_res);
    }

    #[rstest]
    #[case::empty(
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        vec![],
    )]
    #[case::no_prev_asset_weights(
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::from([
            ("eth.axl".to_string(), Decimal::percent(40)),
            ("eth.wh".to_string(), Decimal::percent(40)),
            ("wsteth.axl".to_string(), Decimal::percent(20)),
        ]),
        BTreeMap::new(),
        vec![
            (Scope::denom("eth.axl"), (Decimal::zero(), Decimal::percent(40))),
            (Scope::denom("eth.wh"), (Decimal::zero(), Decimal::percent(40))),
            (Scope::denom("wsteth.axl"), (Decimal::zero(), Decimal::percent(20))),
        ],
    )]
    #[case::no_updated_asset_weights(
        BTreeMap::from([
            ("eth.axl".to_string(), Decimal::percent(20)),
            ("eth.wh".to_string(), Decimal::percent(60)),
            ("wsteth.axl".to_string(), Decimal::percent(20)),
        ]),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        vec![
            (Scope::denom("eth.axl"), (Decimal::percent(20), Decimal::zero())),
            (Scope::denom("eth.wh"), (Decimal::percent(60), Decimal::zero())),
            (Scope::denom("wsteth.axl"), (Decimal::percent(20), Decimal::zero())),
        ],
    )]
    #[case(
        BTreeMap::from([
            ("eth.axl".to_string(), Decimal::percent(20)),
            ("eth.wh".to_string(), Decimal::percent(60)),
            ("wsteth.axl".to_string(), Decimal::percent(20)),
        ]),
        BTreeMap::from([
            ("axelar".to_string(), Decimal::percent(40)),
            ("wormhole".to_string(), Decimal::percent(60)),
        ]),
        BTreeMap::from([
            ("eth.axl".to_string(), Decimal::percent(40)),
            ("eth.wh".to_string(), Decimal::percent(40)),
            ("wsteth.axl".to_string(), Decimal::percent(20)),
        ]),
        BTreeMap::from([
            ("axelar".to_string(), Decimal::percent(60)),
            ("wormhole".to_string(), Decimal::percent(40)),
        ]),
        vec![
            (Scope::denom("eth.axl"), (Decimal::percent(20), Decimal::percent(40))),
            (Scope::denom("eth.wh"), (Decimal::percent(60), Decimal::percent(40))),
            (Scope::denom("wsteth.axl"), (Decimal::percent(20), Decimal::percent(20))),
            (Scope::asset_group("axelar"), (Decimal::percent(40), Decimal::percent(60))),
            (Scope::asset_group("wormhole"), (Decimal::percent(60), Decimal::percent(40))),
        ],
    )]
    fn test_construct_scope_value_pairs(
        #[case] prev_asset_weights: BTreeMap<String, Decimal>,
        #[case] prev_asset_group_weights: BTreeMap<String, Decimal>,
        #[case] updated_asset_weights: BTreeMap<String, Decimal>,
        #[case] updated_asset_group_weights: BTreeMap<String, Decimal>,
        #[case] expected_scope_value_pairs: Vec<(Scope, (Decimal, Decimal))>,
    ) {
        let mut scope_value_pairs = construct_scope_value_pairs(
            prev_asset_weights,
            updated_asset_weights,
            prev_asset_group_weights,
            updated_asset_group_weights,
        )
        .unwrap();

        // assert by disregard order
        scope_value_pairs.sort_by_key(|(scope, _)| scope.key());
        let mut expected_scope_value_pairs = expected_scope_value_pairs;
        expected_scope_value_pairs.sort_by_key(|(scope, _)| scope.key());

        assert_eq!(scope_value_pairs, expected_scope_value_pairs);
    }

    #[test]
    fn test_clean_up_drained_corrupted_assets_group() {
        let sender = Addr::unchecked("addr1");
        let mut deps = cosmwasm_std::testing::mock_dependencies_with_balances(&[(
            sender.to_string().as_str(),
            &[coin(2000000000000u128, "alloyed")],
        )]);

        let transmuter = Transmuter::new();
        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, 100u128.into())
            .unwrap();

        let init_pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::from(1000000000000u128), "denom2", 10u128).unwrap(),
                Asset::new(Uint128::from(1000000000000u128), "denom3", 100u128).unwrap(),
            ],
            asset_groups: BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["denom2".to_string(), "denom3".to_string()])
                    .mark_as_corrupted()
                    .clone(),
            )]),
        };
        transmuter.pool.save(&mut deps.storage, &init_pool).unwrap();

        // Add a rebalancing config for the group
        transmuter
            .rebalancer
            .add_config(
                &mut deps.storage,
                Scope::asset_group("group1"),
                RebalancingConfig::limit_only(Decimal::percent(60)).unwrap(),
            )
            .unwrap();

        let mut pool = transmuter.pool.load(&deps.storage).unwrap();
        let res = transmuter.clean_up_drained_corrupted_assets(&mut deps.storage, &mut pool);
        assert_eq!(res, Ok(()));

        pool = transmuter.pool.load(&deps.storage).unwrap();
        assert_eq!(pool, init_pool);

        pool.exit_pool(&[coin(1000000000000u128, "denom2")])
            .unwrap();
        transmuter.pool.save(&mut deps.storage, &pool).unwrap();

        let res = transmuter.clean_up_drained_corrupted_assets(&mut deps.storage, &mut pool);
        assert_eq!(res, Ok(()));

        let expected_pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::from(1000000000000u128), "denom3", 100u128).unwrap(),
            ],
            asset_groups: BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["denom3".to_string()])
                    .mark_as_corrupted()
                    .clone(),
            )]),
        };
        assert_eq!(pool, expected_pool);

        // Check that the rebalancing config for group1 is still exists
        let rebalancing_configs = transmuter
            .rebalancer
            .get_config_by_scope(&deps.storage, &Scope::asset_group("group1"))
            .unwrap();
        assert!(rebalancing_configs.is_some());

        // Save the updated pool
        transmuter.pool.save(&mut deps.storage, &pool).unwrap();

        pool.exit_pool(&[coin(1000000000000u128, "denom3")])
            .unwrap();

        let res = transmuter.clean_up_drained_corrupted_assets(&mut deps.storage, &mut pool);
        assert_eq!(res, Ok(()));

        let expected_pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
            ],
            asset_groups: BTreeMap::new(),
        };
        assert_eq!(pool, expected_pool);

        // Check that the rebalancing config for group1 is removed
        let rebalancing_configs = transmuter
            .rebalancer
            .get_config_by_scope(&deps.storage, &Scope::asset_group("group1"))
            .unwrap();
        assert_eq!(rebalancing_configs, None);
    }

    #[test]
    fn test_clean_up_drained_corrupted_assets_group_not_corrupted() {
        let mut deps = mock_dependencies();
        let transmuter = Transmuter::new();

        // Initialize the pool with non-corrupted assets and groups
        let init_pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::from(1000000000000u128), "denom2", 10u128).unwrap(),
                Asset::new(Uint128::from(1000000000000u128), "denom3", 100u128).unwrap(),
            ],
            asset_groups: BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["denom2".to_string(), "denom3".to_string()]),
            )]),
        };

        transmuter.pool.save(&mut deps.storage, &init_pool).unwrap();

        // Register a limiter for the group
        transmuter
            .rebalancer
            .add_config(
                &mut deps.storage,
                Scope::asset_group("group1"),
                RebalancingConfig::limit_only(Decimal::one()).unwrap(),
            )
            .unwrap();

        let mut pool = transmuter.pool.load(&deps.storage).unwrap();
        assert_eq!(pool, init_pool);

        // Drain denom2 from the pool
        pool.exit_pool(&[coin(1000000000000u128, "denom2")])
            .unwrap();
        transmuter.pool.save(&mut deps.storage, &pool).unwrap();

        let res = transmuter.clean_up_drained_corrupted_assets(&mut deps.storage, &mut pool);
        assert_eq!(res, Ok(()));

        // Check that the pool remains unchanged
        let expected_pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::zero(), "denom2", 10u128).unwrap(),
                Asset::new(Uint128::from(1000000000000u128), "denom3", 100u128).unwrap(),
            ],
            asset_groups: BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["denom2".to_string(), "denom3".to_string()]),
            )]),
        };
        assert_eq!(pool, expected_pool);

        // Check that the rebalancing config for group1 is still registered
        let rebalancing_configs = transmuter
            .rebalancer
            .get_config_by_scope(&deps.storage, &Scope::asset_group("group1"))
            .unwrap();
        assert!(rebalancing_configs.is_some());

        // Save the updated pool
        transmuter.pool.save(&mut deps.storage, &pool).unwrap();

        // Drain denom3 from the pool
        pool.exit_pool(&[coin(1000000000000u128, "denom3")])
            .unwrap();

        let res = transmuter.clean_up_drained_corrupted_assets(&mut deps.storage, &mut pool);
        assert_eq!(res, Ok(()));

        // Check that the pool remains unchanged except for the drained assets
        let expected_pool = TransmuterPool {
            pool_assets: vec![
                Asset::new(Uint128::from(1000000000000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::zero(), "denom2", 10u128).unwrap(),
                Asset::new(Uint128::zero(), "denom3", 100u128).unwrap(),
            ],
            asset_groups: BTreeMap::from([(
                "group1".to_string(),
                AssetGroup::new(vec!["denom2".to_string(), "denom3".to_string()]),
            )]),
        };
        assert_eq!(pool, expected_pool);

        // Check that the rebalancing config for group1 is still registered
        let rebalancing_configs = transmuter
            .rebalancer
            .get_config_by_scope(&deps.storage, &Scope::asset_group("group1"))
            .unwrap();
        assert!(rebalancing_configs.is_some());
    }

    fn setup_fee_deduction_test() -> (Addr, OwnedDeps<MockStorage, MockApi, MockQuerier>) {
        let sender = Addr::unchecked("sender");
        let mut deps = cosmwasm_std::testing::mock_dependencies_with_balances(&[(
            sender.to_string().as_str(),
            &[coin(20_000_000_000_000u128, "alloyed")],
        )]);

        let transmuter = Transmuter::new();
        transmuter
            .alloyed_asset
            .set_alloyed_denom(&mut deps.storage, &"alloyed".to_string())
            .unwrap();

        transmuter
            .alloyed_asset
            .set_normalization_factor(&mut deps.storage, 100u128.into())
            .unwrap();

        transmuter
            .pool
            .save(
                &mut deps.storage,
                &TransmuterPool {
                    pool_assets: vec![
                        Asset::new(Uint128::from(100_000_000_000u128), "denom1", 1u128).unwrap(), // normalized = 10_000_000_000_000
                        Asset::new(Uint128::from(500_000_000_000u128), "denom2", 10u128).unwrap(), // normalized = 5_000_000_000_000
                        Asset::new(Uint128::from(5_000_000_000_000u128), "denom3", 100u128) // normalized = 5_000_000_000_000
                            .unwrap(),
                    ],
                    asset_groups: BTreeMap::new(),
                },
            )
            .unwrap();

        let mut pool = transmuter.pool.load(&deps.storage).unwrap();
        pool.create_asset_group(
            "group1".to_string(),
            vec!["denom2".to_string(), "denom3".to_string()],
        )
        .unwrap();
        transmuter.pool.save(&mut deps.storage, &pool).unwrap();

        transmuter
            .rebalancer
            .add_config(
                &mut deps.storage,
                Scope::denom("denom1"),
                RebalancingConfig::new(
                    Decimal::percent(50),
                    Decimal::percent(45),
                    Decimal::percent(55),
                    Decimal::percent(30),
                    Decimal::percent(65),
                    Decimal::percent(10),
                    Decimal::percent(20),
                )
                .unwrap(),
            )
            .unwrap();

        transmuter
            .rebalancer
            .add_config(
                &mut deps.storage,
                Scope::asset_group("group1"),
                RebalancingConfig::new(
                    Decimal::percent(55),
                    Decimal::percent(45),
                    Decimal::percent(60),
                    Decimal::percent(30),
                    Decimal::percent(65),
                    Decimal::percent(10),
                    Decimal::percent(20),
                )
                .unwrap(),
            )
            .unwrap();

        transmuter
            .incentive_pool
            .add_tokens(&mut deps.storage, &coin(100_000_000_000, "denom1"))
            .unwrap();
        transmuter
            .incentive_pool
            .add_tokens(&mut deps.storage, &coin(1_000_000_000_000, "denom2"))
            .unwrap();
        transmuter
            .incentive_pool
            .add_tokens(&mut deps.storage, &coin(10_000_000_000_000, "denom3"))
            .unwrap();

        return (sender, deps);
    }

    #[test]
    fn test_swap_non_alloyed_exact_amount_out_with_fee_deduction() {
        let (sender, mut deps) = setup_fee_deduction_test();
        let transmuter = Transmuter::new();

        let mut incentive_pool_balances: Coins = transmuter
            .incentive_pool
            .get_all_pool_balances(&deps.storage)
            .unwrap()
            .try_into()
            .unwrap();

        // the swap makes denom1 60%, denom2 40%
        let token_in_denom = "denom1";

        // fee(group1) = 20_000_000_000_000u128 * 5% * 10% = 100_000_000_000u128 / 100
        // fee(denom1) = 20_000_000_000_000u128 * (5% * 10% + 5% * 20%) = 300_000_000_000u128 / 100
        let amount_in_before_fee = Uint128::from(20_000_000_000u128);
        let fee = Uint128::from(1_000_000_000u128) + Uint128::from(3_000_000_000u128);
        let token_out = coin(200_000_000_000u128, "denom2");
        let token_in_amount = amount_in_before_fee + fee;

        let res = transmuter.swap_non_alloyed_exact_amount_out(
            token_in_denom,
            token_in_amount - Uint128::from(1u128),
            token_out.clone(),
            sender.clone(),
            deps.as_mut(),
        );

        assert_eq!(
            res,
            Err(ContractError::ExcessiveRequiredTokenIn {
                limit: token_in_amount - Uint128::from(1u128),
                required: token_in_amount,
            })
        );

        let res = transmuter.swap_non_alloyed_exact_amount_out(
            token_in_denom,
            token_in_amount,
            token_out.clone(),
            sender.clone(),
            deps.as_mut(),
        );

        let pool = transmuter.pool.load(&deps.storage).unwrap();

        assert_eq!(
            pool.get_pool_asset_by_denom("denom1").unwrap().amount(),
            Uint128::from(100_000_000_000u128) + amount_in_before_fee
        );
        assert_eq!(
            pool.get_pool_asset_by_denom("denom2").unwrap().amount(),
            Uint128::from(500_000_000_000u128) - amount_in_before_fee * Uint128::from(10u128)
        );

        incentive_pool_balances
            .add(coin(fee.u128(), "denom1"))
            .unwrap();

        let updated_incentive_pool_balances: Coins = transmuter
            .incentive_pool
            .get_all_pool_balances(&deps.storage)
            .unwrap()
            .try_into()
            .unwrap();

        assert_eq!(updated_incentive_pool_balances, incentive_pool_balances);

        let response = res.unwrap();
        let data: SwapExactAmountOutResponseData = from_json(&response.data.unwrap()).unwrap();
        assert_eq!(
            data,
            SwapExactAmountOutResponseData {
                token_in_amount: amount_in_before_fee + fee,
            }
        );

        let credits = transmuter
            .incentive_pool
            .get_all_incentive_credits(&deps.storage, None, None)
            .unwrap();
        assert_eq!(credits, vec![]);

        // swap back with the same amount
        let res = transmuter
            .swap_non_alloyed_exact_amount_in(
                token_out,
                "denom1",
                amount_in_before_fee,
                sender.clone(),
                deps.as_mut(),
            )
            .unwrap();
        let data: SwapExactAmountInResponseData = from_json(&res.data.unwrap()).unwrap();
        assert_eq!(
            data,
            SwapExactAmountInResponseData {
                token_out_amount: amount_in_before_fee,
            }
        );

        let pool = transmuter.pool.load(&deps.storage).unwrap();

        assert_eq!(
            pool.pool_assets,
            vec![
                Asset::new(Uint128::from(100_000_000_000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::from(500_000_000_000u128), "denom2", 10u128).unwrap(),
                Asset::new(Uint128::from(5_000_000_000_000u128), "denom3", 100u128).unwrap(),
            ]
        );

        let credits = transmuter
            .incentive_pool
            .get_all_incentive_credits(&deps.storage, None, None)
            .unwrap();
        assert_eq!(
            credits,
            vec![(sender.clone(), fee * Uint128::from(100u128))]
        );

        let pool_denom_factors = pool
            .pool_assets
            .iter()
            .map(|asset| (asset.denom().to_string(), asset.normalization_factor()))
            .collect::<BTreeMap<_, _>>();

        let err = transmuter
            .incentive_pool
            .redeem_incentive(
                &mut deps.storage,
                &sender,
                vec![coin(fee.u128() + 1, "denom1")],
                &pool_denom_factors,
            )
            .unwrap_err();

        assert_eq!(
            err,
            ContractError::InsufficientIncentiveCredit {
                user: sender.clone(),
                available: fee * Uint128::from(100u128),
                requested: (fee + Uint128::from(1u128)) * Uint128::from(100u128),
            }
        );

        transmuter
            .incentive_pool
            .redeem_incentive(
                &mut deps.storage,
                &sender,
                vec![coin(fee.u128(), "denom1")],
                &pool_denom_factors,
            )
            .unwrap();

        incentive_pool_balances
            .sub(coin(fee.u128(), "denom1"))
            .unwrap();

        let credits = transmuter
            .incentive_pool
            .get_all_incentive_credits(&deps.storage, None, None)
            .unwrap();

        assert_eq!(credits, vec![]);

        let updated_incentive_pool_balances = transmuter
            .incentive_pool
            .get_all_pool_balances(&deps.storage)
            .unwrap()
            .try_into()
            .unwrap();

        assert_eq!(incentive_pool_balances, updated_incentive_pool_balances);
    }

    #[test]
    fn test_swap_non_alloyed_exact_amount_in_with_fee_deduction() {
        let (sender, mut deps) = setup_fee_deduction_test();
        let transmuter = Transmuter::new();

        let mut incentive_pool_balances: Coins = transmuter
            .incentive_pool
            .get_all_pool_balances(&deps.storage)
            .unwrap()
            .try_into()
            .unwrap();

        // the swap makes denom1 60%, denom2 40%
        // fee(group1) = 20_000_000_000_000u128 * 5% * 10% = 100_000_000_000u128 / 10
        // fee(denom1) = 20_000_000_000_000u128 * (5% * 10% + 5% * 20%) = 300_000_000_000u128 / 10
        let amount_out_before_fee = Uint128::from(200_000_000_000u128);
        let fee = Uint128::from(10_000_000_000u128) + Uint128::from(30_000_000_000u128);
        let token_in = coin(20_000_000_000u128, "denom1");
        let token_out_amount = amount_out_before_fee - fee;

        let res = transmuter.swap_non_alloyed_exact_amount_in(
            token_in.clone(),
            "denom2",
            token_out_amount + Uint128::from(1u128),
            sender.clone(),
            deps.as_mut(),
        );

        assert_eq!(
            res,
            Err(ContractError::InsufficientTokenOut {
                min_required: token_out_amount + Uint128::from(1u128),
                amount_out: token_out_amount,
            })
        );

        let res = transmuter.swap_non_alloyed_exact_amount_in(
            token_in.clone(),
            "denom2",
            token_out_amount,
            sender.clone(),
            deps.as_mut(),
        );

        let pool = transmuter.pool.load(&deps.storage).unwrap();

        assert_eq!(
            pool.get_pool_asset_by_denom("denom1").unwrap().amount(),
            Uint128::from(100_000_000_000u128) + amount_out_before_fee / Uint128::from(10u128)
        );
        assert_eq!(
            pool.get_pool_asset_by_denom("denom2").unwrap().amount(),
            Uint128::from(500_000_000_000u128) - amount_out_before_fee
        );

        incentive_pool_balances
            .add(coin(fee.u128(), "denom2"))
            .unwrap();

        let updated_incentive_pool_balances = transmuter
            .incentive_pool
            .get_all_pool_balances(&deps.storage)
            .unwrap()
            .try_into()
            .unwrap();

        assert_eq!(incentive_pool_balances, updated_incentive_pool_balances);

        let response = res.unwrap();
        let data: SwapExactAmountInResponseData = from_json(&response.data.unwrap()).unwrap();
        assert_eq!(
            data,
            SwapExactAmountInResponseData {
                token_out_amount: amount_out_before_fee - fee,
            }
        );

        let credits = transmuter
            .incentive_pool
            .get_all_incentive_credits(&deps.storage, None, None)
            .unwrap();
        assert_eq!(credits, vec![]);

        // swap back with the same amount
        let res = transmuter.swap_non_alloyed_exact_amount_out(
            "denom2",
            amount_out_before_fee,
            token_in,
            sender.clone(),
            deps.as_mut(),
        );

        let data: SwapExactAmountOutResponseData = from_json(&res.unwrap().data.unwrap()).unwrap();
        assert_eq!(
            data,
            SwapExactAmountOutResponseData {
                token_in_amount: amount_out_before_fee,
            }
        );

        let pool = transmuter.pool.load(&deps.storage).unwrap();

        assert_eq!(
            pool.pool_assets,
            vec![
                Asset::new(Uint128::from(100_000_000_000u128), "denom1", 1u128).unwrap(),
                Asset::new(Uint128::from(500_000_000_000u128), "denom2", 10u128).unwrap(),
                Asset::new(Uint128::from(5_000_000_000_000u128), "denom3", 100u128).unwrap(),
            ]
        );

        let credits = transmuter
            .incentive_pool
            .get_all_incentive_credits(&deps.storage, None, None)
            .unwrap();

        assert_eq!(credits, vec![(sender.clone(), fee * Uint128::from(10u128))]);

        let pool_denom_factors = pool
            .pool_assets
            .iter()
            .map(|asset| (asset.denom().to_string(), asset.normalization_factor()))
            .collect::<BTreeMap<_, _>>();

        let err = transmuter
            .incentive_pool
            .redeem_incentive(
                &mut deps.storage,
                &sender,
                vec![coin(fee.u128() + 1, "denom2")],
                &pool_denom_factors,
            )
            .unwrap_err();

        assert_eq!(
            err,
            ContractError::InsufficientIncentiveCredit {
                user: sender.clone(),
                available: fee * Uint128::from(10u128),
                requested: (fee + Uint128::from(1u128)) * Uint128::from(10u128),
            }
        );

        transmuter
            .incentive_pool
            .redeem_incentive(
                &mut deps.storage,
                &sender,
                vec![coin(fee.u128(), "denom2")],
                &pool_denom_factors,
            )
            .unwrap();

        incentive_pool_balances
            .sub(coin(fee.u128(), "denom2"))
            .unwrap();

        let credits = transmuter
            .incentive_pool
            .get_all_incentive_credits(&deps.storage, None, None)
            .unwrap();

        assert_eq!(credits, vec![]);

        let updated_incentive_pool_balances = transmuter
            .incentive_pool
            .get_all_pool_balances(&deps.storage)
            .unwrap()
            .try_into()
            .unwrap();

        assert_eq!(incentive_pool_balances, updated_incentive_pool_balances);
    }

    #[test]
    fn test_swap_tokens_to_alloyed_asset_exact_in_with_fee_deduction_and_incentivization() {
        let transmuter = Transmuter::new();

        let (sender, mut deps) = setup_fee_deduction_test();
        let mut incentive_pool_balances: Coins = transmuter
            .incentive_pool
            .get_all_pool_balances(&deps.storage)
            .unwrap()
            .try_into()
            .unwrap();

        // Multiple tokens in that will make denom1 55%, denom2 25%
        let tokens_in = vec![
            coin(37_500_000_000u128, "denom1"), // contributes 37_500_000_000 * 100 = 3_750_000_000_000 normalized
            coin(125_000_000_000u128, "denom2"), // contributes 125_000_000_000 * 10 = 1_250_000_000_000 normalized
        ];

        // fee(denom1) = 25_000_000_000_000u128 * (5% * 1%) = 12_500_000_000u128
        // fee(group1) = 25_000_000_000_000u128 * 0% = 0
        let amount_out_before_fee = Uint128::from(3_750_000_000_000 + 1_250_000_000_000u128);
        let fee = Uint128::from(125_000_000_000u128);
        let token_out_amount = amount_out_before_fee - fee;

        let res = transmuter.swap_tokens_to_alloyed_asset(
            Entrypoint::Exec,
            SwapToAlloyedConstraint::ExactIn {
                tokens_in: &tokens_in,
                token_out_min_amount: token_out_amount + Uint128::from(1u128),
            },
            sender.clone(),
            deps.as_mut(),
            mock_env(),
        );

        assert_eq!(
            res,
            Err(ContractError::InsufficientTokenOut {
                min_required: token_out_amount + Uint128::from(1u128),
                amount_out: token_out_amount,
            })
        );

        let res = transmuter.swap_tokens_to_alloyed_asset(
            Entrypoint::Exec,
            SwapToAlloyedConstraint::ExactIn {
                tokens_in: &tokens_in,
                token_out_min_amount: token_out_amount,
            },
            sender.clone(),
            deps.as_mut(),
            mock_env(),
        );

        let messages = res
            .unwrap()
            .messages
            .into_iter()
            .map(|m| {
                let CosmosMsg::Stargate { value, .. } = m.msg else {
                    panic!("must be Startgate message")
                };
                MsgMint::decode(value.as_slice()).unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            messages,
            vec![
                MsgMint {
                    amount: Some(coin(fee.u128(), "alloyed").into()),
                    mint_to_address: MOCK_CONTRACT_ADDR.to_string(),
                    sender: MOCK_CONTRACT_ADDR.to_string(),
                },
                MsgMint {
                    amount: Some(coin(token_out_amount.u128(), "alloyed").into()),
                    mint_to_address: sender.to_string(),
                    sender: MOCK_CONTRACT_ADDR.to_string(),
                }
            ]
        );

        let pool = transmuter.pool.load(&deps.storage).unwrap();

        // Verify pool state after join
        assert_eq!(
            pool.get_pool_asset_by_denom("denom1").unwrap().amount(),
            Uint128::from(100_000_000_000u128) + tokens_in[0].amount
        );
        assert_eq!(
            pool.get_pool_asset_by_denom("denom2").unwrap().amount(),
            Uint128::from(500_000_000_000u128) + tokens_in[1].amount
        );

        incentive_pool_balances
            .add(coin(fee.u128(), "alloyed"))
            .unwrap();

        // Verify fee is collected (minted to contract)
        let updated_incentive_pool_balances: Coins = transmuter
            .incentive_pool
            .get_all_pool_balances(&deps.storage)
            .unwrap()
            .try_into()
            .unwrap();

        // Fee should be collected in alloyed asset
        assert_eq!(incentive_pool_balances, updated_incentive_pool_balances);

        let credits = transmuter
            .incentive_pool
            .get_all_incentive_credits(&deps.storage, None, None)
            .unwrap();
        assert_eq!(credits, vec![]);

        // rebalance it back

        let res = transmuter
            .swap_tokens_to_alloyed_asset(
                Entrypoint::Exec,
                SwapToAlloyedConstraint::ExactIn {
                    tokens_in: &[coin(amount_out_before_fee.u128(), "denom3")],
                    token_out_min_amount: amount_out_before_fee,
                },
                sender.clone(),
                deps.as_mut(),
                mock_env(),
            )
            .unwrap();

        let messages = res
            .messages
            .into_iter()
            .map(|m| {
                let CosmosMsg::Stargate { value, .. } = m.msg else {
                    panic!("must be Startgate message")
                };
                MsgMint::decode(value.as_slice()).unwrap()
            })
            .collect::<Vec<_>>();

        assert_eq!(
            messages,
            vec![MsgMint {
                amount: Some(coin(amount_out_before_fee.u128(), "alloyed").into()),
                mint_to_address: sender.to_string(),
                sender: MOCK_CONTRACT_ADDR.to_string(),
            }]
        );

        // check pool state
        let pool = transmuter.pool.load(&deps.storage).unwrap();
        assert_eq!(
            pool.get_pool_asset_by_denom("denom3").unwrap().amount(),
            Uint128::from(5_000_000_000_000u128) + amount_out_before_fee
        );

        // check incentive pool state
        let updated_incentive_pool_balances: Coins = transmuter
            .incentive_pool
            .get_all_pool_balances(&deps.storage)
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(incentive_pool_balances, updated_incentive_pool_balances);
    }

    #[test]
    fn test_swap_tokens_to_alloyed_asset_exact_out_with_fee_deduction_and_incentivization() {
        let transmuter = Transmuter::new();

        let (sender, mut deps) = setup_fee_deduction_test();
        let mut incentive_pool_balances: Coins = transmuter
            .incentive_pool
            .get_all_pool_balances(&deps.storage)
            .unwrap()
            .try_into()
            .unwrap();

        // fee(denom1) = 25_000_000_000_000u128 * ((5% * 1%) + (5% * 2%)) = 37_500_000_000
        // fee(group1) = 25_000_000_000_000u128 * (5% * 1%) = 12_500_000_000
        // = 50_000_000_000u128
        let res = transmuter.swap_tokens_to_alloyed_asset(
            Entrypoint::Exec,
            SwapToAlloyedConstraint::ExactOut {
                token_in_denom: "denom1",
                token_in_max_amount: Uint128::from(55_000_000_000u128 - 1),
                token_out_amount: Uint128::from(5_000_000_000_000u128),
            },
            sender.clone(),
            deps.as_mut(),
            mock_env(),
        );

        assert_eq!(
            res,
            Err(ContractError::ExcessiveRequiredTokenIn {
                limit: Uint128::from(55_000_000_000u128 - 1),
                required: Uint128::from(55_000_000_000u128),
            })
        );

        let token_out_amount = Uint128::from(5_000_000_000_000u128);
        let amount_in_before_fee = Uint128::from(50_000_000_000u128); // 5_000_000_000_000 / 100
        let fee = Uint128::from(5_000_000_000u128); // 50_000_000_000 based on the fee calculation
        let token_in_amount = amount_in_before_fee + fee;

        let res = transmuter.swap_tokens_to_alloyed_asset(
            Entrypoint::Exec,
            SwapToAlloyedConstraint::ExactOut {
                token_in_denom: "denom1",
                token_in_max_amount: token_in_amount,
                token_out_amount,
            },
            sender.clone(),
            deps.as_mut(),
            mock_env(),
        );

        let messages = res
            .unwrap()
            .messages
            .into_iter()
            .map(|m| {
                let CosmosMsg::Stargate { value, .. } = m.msg else {
                    panic!("must be Startgate message")
                };
                MsgMint::decode(value.as_slice()).unwrap()
            })
            .collect::<Vec<_>>();

        assert_eq!(
            messages,
            vec![MsgMint {
                amount: Some(coin(token_out_amount.u128(), "alloyed").into()),
                mint_to_address: sender.to_string(),
                sender: MOCK_CONTRACT_ADDR.to_string(),
            }]
        );

        let pool = transmuter.pool.load(&deps.storage).unwrap();

        // Verify pool state after join
        assert_eq!(
            pool.get_pool_asset_by_denom("denom1").unwrap().amount(),
            Uint128::from(100_000_000_000u128) + amount_in_before_fee
        );

        incentive_pool_balances
            .add(coin(fee.u128(), "denom1"))
            .unwrap();

        // Verify fee is collected
        let updated_incentive_pool_balances: Coins = transmuter
            .incentive_pool
            .get_all_pool_balances(&deps.storage)
            .unwrap()
            .try_into()
            .unwrap();

        // Fee should be collected in denom1
        assert_eq!(incentive_pool_balances, updated_incentive_pool_balances);

        let credits = transmuter
            .incentive_pool
            .get_all_incentive_credits(&deps.storage, None, None)
            .unwrap();
        assert_eq!(credits, vec![]);

        // Rebalance back by refilling the same amount for denom2 (which is group1)
        let res = transmuter
            .swap_tokens_to_alloyed_asset(
                Entrypoint::Exec,
                SwapToAlloyedConstraint::ExactOut {
                    token_in_denom: "denom2",
                    token_in_max_amount: Uint128::from(500_000_000_000u128),
                    token_out_amount: Uint128::from(5_000_000_000_000u128),
                },
                sender.clone(),
                deps.as_mut(),
                mock_env(),
            )
            .unwrap();

        let messages = res
            .messages
            .into_iter()
            .map(|m| {
                let CosmosMsg::Stargate { value, .. } = m.msg else {
                    panic!("must be Startgate message")
                };
                MsgMint::decode(value.as_slice()).unwrap()
            })
            .collect::<Vec<_>>();

        assert_eq!(
            messages,
            vec![MsgMint {
                amount: Some(coin(token_out_amount.u128(), "alloyed").into()),
                mint_to_address: sender.to_string(),
                sender: MOCK_CONTRACT_ADDR.to_string(),
            }]
        );

        // check for pool state
        let pool = transmuter.pool.load(&deps.storage).unwrap();
        assert_eq!(
            pool.get_pool_asset_by_denom("denom2").unwrap().amount(),
            Uint128::from(500_000_000_000u128 + 500_000_000_000u128)
        );

        // check for incentive credit
        let credits = transmuter
            .incentive_pool
            .get_all_incentive_credits(&deps.storage, None, None)
            .unwrap();
        assert_eq!(credits, vec![(sender, Uint128::from(500_000_000_000u128))]);
    }

    #[test]
    fn test_swap_alloyed_asset_to_tokens_exact_in_with_fee_deduction_and_incentivization() {
        let transmuter = Transmuter::new();

        let (sender, mut deps) = setup_fee_deduction_test();
        let mut incentive_pool_balance: Coins = transmuter
            .incentive_pool
            .get_all_pool_balances(&deps.storage)
            .unwrap()
            .try_into()
            .unwrap();

        // 2_000_000_000_000 alloyed in -> denom1

        // fee(denom1) = 20_000_000_000_000 * (5% * 1%) = 100_000_000_000
        // fee(group1) = 20_000_000_000_000 * (5% * 1%) = 100_000_000_000
        let token_in_amount = Uint128::from(2_000_000_000_000u128);
        let token_out_amount_before_fee = Uint128::from(20_000_000_000u128);
        let fee = Uint128::from(222_222_223u128);
        let token_out_amount = token_out_amount_before_fee - fee;

        let res = transmuter.swap_alloyed_asset_to_tokens(
            Entrypoint::Exec,
            SwapFromAlloyedConstraint::ExactIn {
                token_out_denom: "denom1",
                token_out_min_amount: token_out_amount + Uint128::from(1u128),
                token_in_amount,
            },
            BurnTarget::SenderAccount {},
            sender.clone(),
            deps.as_mut(),
            mock_env(),
        );

        assert_eq!(
            res,
            Err(ContractError::InsufficientTokenOut {
                min_required: token_out_amount + Uint128::from(1u128),
                amount_out: token_out_amount,
            })
        );

        let res = transmuter
            .swap_alloyed_asset_to_tokens(
                Entrypoint::Exec,
                SwapFromAlloyedConstraint::ExactIn {
                    token_out_denom: "denom1",
                    token_out_min_amount: token_out_amount,
                    token_in_amount,
                },
                BurnTarget::SenderAccount {},
                sender.clone(),
                deps.as_mut(),
                mock_env(),
            )
            .unwrap();

        let burn_msg = res
            .messages
            .get(0)
            .cloned()
            .map(|m| {
                let CosmosMsg::Stargate { value, .. } = m.msg else {
                    panic!("must be Startgate message")
                };
                MsgBurn::decode(value.as_slice()).unwrap()
            })
            .unwrap();

        assert_eq!(
            burn_msg,
            MsgBurn {
                burn_from_address: sender.to_string(),
                amount: Some(coin(token_in_amount.u128(), "alloyed").into()),
                sender: MOCK_CONTRACT_ADDR.to_string(),
            }
        );

        let send_msg = res.messages.get(1).cloned().unwrap();

        assert_eq!(
            send_msg,
            SubMsg::new(BankMsg::Send {
                to_address: sender.to_string(),
                amount: vec![coin(token_out_amount.u128(), "denom1")],
            })
        );

        assert_eq!(res.messages.len(), 2);

        let pool = transmuter.pool.load(&deps.storage).unwrap();

        // Verify pool state after exit
        assert_eq!(
            pool.get_pool_asset_by_denom("denom1").unwrap().amount(),
            Uint128::from(100_000_000_000u128) - token_out_amount_before_fee
        );
        assert_eq!(
            pool.get_pool_asset_by_denom("denom2").unwrap().amount(),
            Uint128::from(500_000_000_000u128)
        );
        assert_eq!(
            pool.get_pool_asset_by_denom("denom3").unwrap().amount(),
            Uint128::from(5_000_000_000_000u128)
        );

        // Verify fee is collected (minted to contract)
        let updated_incentive_pool_balances: Coins = transmuter
            .incentive_pool
            .get_all_pool_balances(&deps.storage)
            .unwrap()
            .try_into()
            .unwrap();

        incentive_pool_balance
            .add(coin(fee.u128(), "denom1"))
            .unwrap();

        // Fee should be collected in alloyed asset
        assert_eq!(incentive_pool_balance, updated_incentive_pool_balances);

        let credits = transmuter
            .incentive_pool
            .get_all_incentive_credits(&deps.storage, None, None)
            .unwrap();
        assert_eq!(credits, vec![]);

        // rebalance it back

        let tokens_out = vec![coin(200_000_000_000u128, "denom2")];

        let res = transmuter
            .swap_alloyed_asset_to_tokens(
                Entrypoint::Exec,
                SwapFromAlloyedConstraint::ExactOut {
                    tokens_out: &tokens_out,
                    token_in_max_amount: Uint128::from(2_000_000_000_000u128),
                },
                BurnTarget::SenderAccount {},
                sender.clone(),
                deps.as_mut(),
                mock_env(),
            )
            .unwrap();

        let burn_msg = res
            .messages
            .get(0)
            .cloned()
            .map(|m| {
                let CosmosMsg::Stargate { value, .. } = m.msg else {
                    panic!("must be Startgate message")
                };
                MsgBurn::decode(value.as_slice()).unwrap()
            })
            .unwrap();

        assert_eq!(
            burn_msg,
            MsgBurn {
                burn_from_address: sender.to_string(),
                amount: Some(coin(2_000_000_000_000u128, "alloyed").into()),
                sender: MOCK_CONTRACT_ADDR.to_string(),
            }
        );

        let send_msg = res.messages.get(1).cloned().unwrap();

        assert_eq!(
            send_msg,
            SubMsg::new(BankMsg::Send {
                to_address: sender.to_string(),
                amount: tokens_out.clone(),
            })
        );

        assert_eq!(res.messages.len(), 2);

        // check pool state
        let pool = transmuter.pool.load(&deps.storage).unwrap();
        assert_eq!(
            pool.get_pool_asset_by_denom("denom2").unwrap().amount(),
            Uint128::from(500_000_000_000u128 - 200_000_000_000u128)
        );

        // check incentive pool state
        let updated_incentive_pool_balances: Coins = transmuter
            .incentive_pool
            .get_all_pool_balances(&deps.storage)
            .unwrap()
            .try_into()
            .unwrap();

        assert_eq!(incentive_pool_balance, updated_incentive_pool_balances);

        let incetive_credits = transmuter
            .incentive_pool
            .get_all_incentive_credits(&deps.storage, None, None)
            .unwrap();
        assert_eq!(
            incetive_credits,
            vec![(sender, Uint128::from(19_999_999_999u128))]
        );
    }

    #[test]
    fn test_swap_alloyed_asset_to_tokens_exact_out_with_fee_deduction_and_incentivization() {
        let transmuter = Transmuter::new();

        let (sender, mut deps) = setup_fee_deduction_test();
        let mut incentive_pool_balances: Coins = transmuter
            .incentive_pool
            .get_all_pool_balances(&deps.storage)
            .unwrap()
            .try_into()
            .unwrap();

        // Multiple tokens out that will make denom1 40%, group1 60%
        let tokens_out = vec![
            coin(80_000_000_000u128, "denom1"), // remove 80_000_000_000u128 * 100 = 8_000_000_000_000 normalized
            coin(300_000_000_000u128, "denom2"), // remove 300_000_000_000u128 * 10 = 3_000_000_000_000 normalized
            coin(4_000_000_000_000u128, "denom3"), // remove 4_000_000_000_000u128 * 1 = 4_000_000_000_000 normalized
        ];

        // fee(denom1) = 20_000_000_000_000 * (5% * 1%) = 100_000_000_000
        // fee(group1) = 20_000_000_000_000 * (5% * 1%) = 100_000_000_000
        let amount_in_before_fee = Uint128::from(15_000_000_000_000u128);
        let fee = Uint128::from(200_000_000_000u128);
        let token_in_amount = amount_in_before_fee + fee;

        let res = transmuter.swap_alloyed_asset_to_tokens(
            Entrypoint::Exec,
            SwapFromAlloyedConstraint::ExactOut {
                tokens_out: &tokens_out,
                token_in_max_amount: token_in_amount - Uint128::from(1u128),
            },
            BurnTarget::SenderAccount {},
            sender.clone(),
            deps.as_mut(),
            mock_env(),
        );

        assert_eq!(
            res,
            Err(ContractError::ExcessiveRequiredTokenIn {
                limit: token_in_amount - Uint128::from(1u128),
                required: token_in_amount,
            })
        );

        let res = transmuter
            .swap_alloyed_asset_to_tokens(
                Entrypoint::Exec,
                SwapFromAlloyedConstraint::ExactOut {
                    tokens_out: &tokens_out,
                    token_in_max_amount: token_in_amount,
                },
                BurnTarget::SenderAccount {},
                sender.clone(),
                deps.as_mut(),
                mock_env(),
            )
            .unwrap();

        let burn_msg = res
            .messages
            .get(0)
            .cloned()
            .map(|m| {
                let CosmosMsg::Stargate { value, .. } = m.msg else {
                    panic!("must be Startgate message")
                };
                MsgBurn::decode(value.as_slice()).unwrap()
            })
            .unwrap();

        assert_eq!(
            burn_msg,
            MsgBurn {
                burn_from_address: sender.to_string(),
                amount: Some(coin(amount_in_before_fee.u128(), "alloyed").into()),
                sender: MOCK_CONTRACT_ADDR.to_string(),
            }
        );

        let send_msg = res.messages.get(1).cloned().unwrap();

        assert_eq!(
            send_msg,
            SubMsg::new(BankMsg::Send {
                to_address: sender.to_string(),
                amount: tokens_out.clone(),
            })
        );

        assert_eq!(res.messages.len(), 2);

        let pool = transmuter.pool.load(&deps.storage).unwrap();

        // Verify pool state after exit
        assert_eq!(
            pool.get_pool_asset_by_denom("denom1").unwrap().amount(),
            Uint128::from(100_000_000_000u128) - tokens_out[0].amount
        );
        assert_eq!(
            pool.get_pool_asset_by_denom("denom2").unwrap().amount(),
            Uint128::from(500_000_000_000u128) - tokens_out[1].amount
        );
        assert_eq!(
            pool.get_pool_asset_by_denom("denom3").unwrap().amount(),
            Uint128::from(5_000_000_000_000u128) - tokens_out[2].amount
        );

        // Verify fee is collected (minted to contract)
        let updated_incentive_pool_balances: Coins = transmuter
            .incentive_pool
            .get_all_pool_balances(&deps.storage)
            .unwrap()
            .try_into()
            .unwrap();

        // Fee should be collected in alloyed asset
        incentive_pool_balances
            .add(coin(fee.u128(), "alloyed"))
            .unwrap();
        assert_eq!(incentive_pool_balances, updated_incentive_pool_balances);

        let credits = transmuter
            .incentive_pool
            .get_all_incentive_credits(&deps.storage, None, None)
            .unwrap();
        assert_eq!(credits, vec![]);

        // rebalance it back

        let tokens_out = vec![coin(100_000_000_000u128, "denom2")];

        let res = transmuter
            .swap_alloyed_asset_to_tokens(
                Entrypoint::Exec,
                SwapFromAlloyedConstraint::ExactOut {
                    tokens_out: &tokens_out,
                    token_in_max_amount: Uint128::from(1_000_000_000_000u128),
                },
                BurnTarget::SenderAccount {},
                sender.clone(),
                deps.as_mut(),
                mock_env(),
            )
            .unwrap();

        let burn_msg = res
            .messages
            .get(0)
            .cloned()
            .map(|m| {
                let CosmosMsg::Stargate { value, .. } = m.msg else {
                    panic!("must be Startgate message")
                };
                MsgBurn::decode(value.as_slice()).unwrap()
            })
            .unwrap();

        assert_eq!(
            burn_msg,
            MsgBurn {
                burn_from_address: sender.to_string(),
                amount: Some(coin(1_000_000_000_000u128, "alloyed").into()),
                sender: MOCK_CONTRACT_ADDR.to_string(),
            }
        );

        let send_msg = res.messages.get(1).cloned().unwrap();

        assert_eq!(
            send_msg,
            SubMsg::new(BankMsg::Send {
                to_address: sender.to_string(),
                amount: tokens_out.clone(),
            })
        );

        assert_eq!(res.messages.len(), 2);

        // check pool state
        let pool = transmuter.pool.load(&deps.storage).unwrap();
        assert_eq!(
            pool.get_pool_asset_by_denom("denom3").unwrap().amount(),
            Uint128::from(1_000_000_000_000u128)
        );

        // check incentive pool state
        let updated_incentive_pool_balances = transmuter
            .incentive_pool
            .get_all_pool_balances(&deps.storage)
            .unwrap()
            .try_into()
            .unwrap();

        assert_eq!(incentive_pool_balances, updated_incentive_pool_balances);

        let incetive_credits = transmuter
            .incentive_pool
            .get_all_incentive_credits(&deps.storage, None, None)
            .unwrap();
        assert_eq!(
            incetive_credits,
            vec![(sender, Uint128::from(50_000_000_000u128))]
        );
    }
}
