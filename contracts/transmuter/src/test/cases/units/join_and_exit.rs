use std::collections::HashMap;

use cosmwasm_std::{attr, coin, Coin, Uint128};
use itertools::Itertools;
use osmosis_test_tube::{Account, OsmosisTestApp};

use crate::{
    asset::AssetConfig,
    contract::sv::{ExecMsg, InstantiateMsg, QueryMsg},
    contract::{
        GetShareDenomResponse, GetSharesResponse, GetTotalPoolLiquidityResponse,
        GetTotalSharesResponse,
    },
    test::test_env::{assert_contract_err, TestEnvBuilder},
    ContractError,
};

#[test]
fn test_join_pool_with_single_lp_should_update_shares_and_liquidity_properly() {
    #[derive(Debug)]
    struct Case {
        funds: Vec<Coin>,
    }

    let cases = vec![
        Case {
            funds: vec![coin(1, "denoma")],
        },
        Case {
            funds: vec![coin(1, "denomb")],
        },
        Case {
            funds: vec![coin(100, "denoma")],
        },
        Case {
            funds: vec![coin(100, "denomb")],
        },
        Case {
            funds: vec![coin(100_000_000_000_000, "denoma")],
        },
        Case {
            funds: vec![coin(100_000_000_000_000, "denomb")],
        },
        Case {
            funds: vec![coin(u128::MAX, "denoma")],
        },
        Case {
            funds: vec![coin(u128::MAX, "denomb")],
        },
        Case {
            funds: vec![coin(999_999_999, "denoma"), coin(999_999_999, "denomb")],
        },
        Case {
            funds: vec![coin(12_000_000_000, "denoma"), coin(999_999_999, "denomb")],
        },
    ];

    for case in cases {
        let app = OsmosisTestApp::new();

        // Find missing denom from funds
        let all_denoms = vec![
            AssetConfig::from_denom_str("denoma"),
            AssetConfig::from_denom_str("denomb"),
        ];
        let funds_denoms = case
            .funds
            .iter()
            .map(|c| c.denom.to_string())
            .unique()
            .collect::<Vec<_>>();

        let missing_denoms = all_denoms
            .into_iter()
            .filter(|info| !funds_denoms.contains(&info.denom))
            .map(|info| info.denom)
            .collect::<Vec<_>>();

        // make supply non-zero
        for denom in missing_denoms {
            app.init_account(&[coin(1, denom)]).unwrap();
        }

        let t = TestEnvBuilder::new()
            .with_account("provider", case.funds.clone())
            .with_instantiate_msg(crate::contract::sv::InstantiateMsg {
                pool_asset_configs: vec![
                    AssetConfig::from_denom_str("denoma"),
                    AssetConfig::from_denom_str("denomb"),
                ],
                alloyed_asset_subdenom: "all".to_string(),
                alloyed_asset_normalization_factor: Uint128::one(),
                admin: None,
                moderator: "osmo1cyyzpxplxdzkeea7kwsydadg87357qnahakaks".to_string(),
            })
            .build(&app);

        t.contract
            .execute(&ExecMsg::JoinPool {}, &case.funds, &t.accounts["provider"])
            .unwrap();

        // check if shares are updated
        let GetSharesResponse { shares } = t
            .contract
            .query(&QueryMsg::GetShares {
                address: t.accounts["provider"].address(),
            })
            .unwrap();

        // shares == sum of cases.funds amount
        assert_eq!(
            shares,
            Uint128::from(
                case.funds
                    .iter()
                    .fold(0, |acc, coin| { acc + coin.amount.u128() })
            ),
            "check if shares are updated: {:?}",
            case
        );

        // check if total shares are updated
        let GetTotalSharesResponse { total_shares } =
            t.contract.query(&QueryMsg::GetTotalShares {}).unwrap();

        assert_eq!(total_shares, shares);

        // check if pool liquidity is updated
        let GetTotalPoolLiquidityResponse {
            total_pool_liquidity,
        } = t
            .contract
            .query(&QueryMsg::GetTotalPoolLiquidity {})
            .unwrap();

        assert_eq!(
            total_pool_liquidity
                .into_iter()
                .filter(|coin| !coin.amount.is_zero())
                .collect::<Vec<_>>(),
            case.funds,
            "check if pool liquidity is updated: {:?}",
            case
        );
    }
}

#[test]
fn test_join_pool_should_update_shares_and_liquidity_properly() {
    #[derive(Clone, Debug)]
    struct Case<'a> {
        joins: Vec<(&'a str, Vec<Coin>)>,
    }

    let cases = vec![
        Case {
            joins: vec![
                ("addr1", vec![coin(1, "denoma")]),
                ("addr2", vec![coin(1, "denomb")]),
            ],
        },
        Case {
            joins: vec![("addr1", vec![coin(u128::MAX, "denoma")])],
        },
        Case {
            joins: vec![("addr1", vec![coin(u128::MAX, "denomb")])],
        },
        Case {
            joins: vec![
                ("addr1", vec![coin(100_000, "denoma")]),
                ("addr2", vec![coin(999_999_999, "denomb")]),
                ("addr3", vec![coin(1, "denoma")]),
                ("addr4", vec![coin(2, "denomb")]),
            ],
        },
        Case {
            joins: vec![
                ("addr1", vec![coin(100_000, "denoma"), coin(999, "denomb")]),
                (
                    "addr2",
                    vec![coin(999_999_999, "denoma"), coin(999_999_999, "denomb")],
                ),
                ("addr3", vec![coin(1, "denoma"), coin(1, "denomb")]),
            ],
        },
    ];

    for case in cases {
        let app = OsmosisTestApp::new();
        let mut builder = TestEnvBuilder::new();

        // Find missing denom from joins
        let all_denoms = vec![
            AssetConfig::from_denom_str("denoma"),
            AssetConfig::from_denom_str("denomb"),
        ];
        let join_denoms = case
            .joins
            .iter()
            .flat_map(|(_, coins)| coins.iter().map(|c| c.denom.to_string()))
            .unique()
            .collect::<Vec<_>>();

        let missing_denoms = all_denoms
            .into_iter()
            .filter(|info| !join_denoms.contains(&info.denom))
            .map(|info| info.denom)
            .collect::<Vec<_>>();

        // make supply non-zero
        for denom in missing_denoms {
            app.init_account(&[coin(1, denom)]).unwrap();
        }

        for (acc, funds) in case.joins.clone() {
            builder = builder.with_account(acc, funds);
        }

        let t = builder
            .with_instantiate_msg(InstantiateMsg {
                pool_asset_configs: vec![
                    AssetConfig::from_denom_str("denoma"),
                    AssetConfig::from_denom_str("denomb"),
                ],
                alloyed_asset_subdenom: "all".to_string(),
                alloyed_asset_normalization_factor: Uint128::one(),
                admin: None,
                moderator: "osmo1cyyzpxplxdzkeea7kwsydadg87357qnahakaks".to_string(),
            })
            .build(&app);

        for (addr, funds) in case.joins.clone() {
            // join pool
            t.contract
                .execute(&ExecMsg::JoinPool {}, &funds, &t.accounts[addr])
                .unwrap();
        }

        // check if total shares are updated
        let GetTotalSharesResponse { total_shares } =
            t.contract.query(&QueryMsg::GetTotalShares {}).unwrap();

        assert_eq!(
            total_shares,
            case.joins
                .clone()
                .into_iter()
                .fold(Uint128::zero(), |acc, (_, funds)| {
                    acc + funds
                        .iter()
                        .fold(Uint128::zero(), |acc, coin| acc + coin.amount)
                })
        );

        // check if pool liquidity is updated
        let GetTotalPoolLiquidityResponse {
            total_pool_liquidity,
        } = t
            .contract
            .query(&QueryMsg::GetTotalPoolLiquidity {})
            .unwrap();

        let total_pool_liquidity: Vec<Coin> = total_pool_liquidity
            .into_iter()
            .filter(|coin| !coin.amount.is_zero())
            .collect();

        let mut expected_pool_liquidity = case
            .joins
            .iter()
            .fold(HashMap::new(), |mut acc, (_, funds)| {
                for coin in funds {
                    let amount = acc.entry(coin.denom.clone()).or_insert_with(Uint128::zero);
                    *amount += coin.amount;
                }
                acc
            })
            .iter()
            .map(|(denom, amount)| Coin {
                denom: denom.clone(),
                amount: *amount,
            })
            .filter(|coin| !coin.amount.is_zero())
            .collect::<Vec<_>>();

        expected_pool_liquidity.sort_by(|a, b| a.denom.cmp(&b.denom));

        assert_eq!(
            total_pool_liquidity, expected_pool_liquidity,
            "checking if pooliquidity is properly updated, case: {:?}",
            case
        );
    }
}

#[test]
fn test_exit_pool_less_than_their_shares_should_update_shares_and_liquidity_properly() {
    #[derive(Clone, Debug)]
    struct Case {
        join: Vec<Coin>,
        exit: Vec<Coin>,
    }

    let cases = vec![
        Case {
            join: vec![coin(1, "denoma")],
            exit: vec![coin(1, "denoma")],
        },
        Case {
            join: vec![coin(100_000, "denoma"), coin(1, "denomb")],
            exit: vec![coin(100_000, "denoma")],
        },
        Case {
            join: vec![coin(1, "denoma"), coin(100_000, "denomb")],
            exit: vec![coin(100_000, "denomb")],
        },
        Case {
            join: vec![coin(u128::MAX, "denoma")],
            exit: vec![coin(u128::MAX, "denoma")],
        },
        Case {
            join: vec![coin(u128::MAX, "denoma")],
            exit: vec![coin(u128::MAX - 1, "denoma")],
        },
        Case {
            join: vec![coin(u128::MAX, "denoma")],
            exit: vec![coin(u128::MAX - 100_000_000, "denoma")],
        },
    ];

    for case in cases {
        let app = OsmosisTestApp::new();

        let t = TestEnvBuilder::new()
            .with_account("instantiator", vec![])
            .with_account(
                "addr1",
                vec![coin(u128::MAX, "denoma"), coin(u128::MAX, "denomb")],
            )
            .with_instantiate_msg(InstantiateMsg {
                pool_asset_configs: vec![
                    AssetConfig::from_denom_str("denoma"),
                    AssetConfig::from_denom_str("denomb"),
                ],
                alloyed_asset_subdenom: "all".to_string(),
                alloyed_asset_normalization_factor: Uint128::one(),
                admin: None,
                moderator: "osmo1cyyzpxplxdzkeea7kwsydadg87357qnahakaks".to_string(),
            })
            .build(&app);

        t.contract
            .execute(&ExecMsg::JoinPool {}, &case.join, &t.accounts["addr1"])
            .unwrap();

        // check if shares are updated
        let GetSharesResponse { shares } = t
            .contract
            .query(&QueryMsg::GetShares {
                address: t.accounts["addr1"].address(),
            })
            .unwrap();

        // shares == sum of cases.funds amount
        assert_eq!(
            shares,
            Uint128::from(
                case.join
                    .iter()
                    .fold(0, |acc, coin| { acc + coin.amount.u128() })
            ),
            "check if shares are updated: {:?}",
            case
        );

        // check if total shares are updated
        let GetTotalSharesResponse { total_shares } =
            t.contract.query(&QueryMsg::GetTotalShares {}).unwrap();

        assert_eq!(total_shares, shares);

        // check if pool liquidity is updated
        let GetTotalPoolLiquidityResponse {
            total_pool_liquidity,
        } = t
            .contract
            .query(&QueryMsg::GetTotalPoolLiquidity {})
            .unwrap();

        let total_pool_liquidity = total_pool_liquidity
            .into_iter()
            .filter(|coin| !coin.amount.is_zero())
            .collect::<Vec<_>>();

        assert_eq!(
            total_pool_liquidity,
            case.join
                .iter()
                .filter(|&coin| !coin.amount.is_zero())
                .cloned()
                .collect::<Vec<Coin>>(),
            "check if pool liquidity is updated: {:?}",
            case
        );

        // exit pool
        let res = t
            .contract
            .execute(
                &ExecMsg::ExitPool {
                    tokens_out: case.exit.clone(),
                },
                &[],
                &t.accounts["addr1"],
            )
            .unwrap();

        // sum of exit amount
        let exit_amount = case
            .exit
            .iter()
            .fold(Uint128::zero(), |acc, coin| acc + coin.amount)
            .u128();

        let burn_attrs = res
            .events
            .into_iter()
            .find(|e| e.ty == "tf_burn")
            .unwrap()
            .attributes;

        let GetShareDenomResponse { share_denom } =
            t.contract.query(&QueryMsg::GetShareDenom {}).unwrap();

        assert_eq!(
            burn_attrs,
            vec![
                attr("burn_from_address", t.accounts["addr1"].address()),
                attr("amount", format!("{}{}", exit_amount, share_denom)),
                attr("msg_index", "0"),
            ]
        );

        let total_exit_amount = case
            .exit
            .iter()
            .fold(Uint128::zero(), |acc, coin| acc + coin.amount);

        // check if shares are updated
        let prev_shares = shares;
        let GetSharesResponse { shares } = t
            .contract
            .query(&QueryMsg::GetShares {
                address: t.accounts["addr1"].address(),
            })
            .unwrap();

        assert_eq!(
            shares,
            prev_shares - total_exit_amount,
            "check if shares are updated after exit: case: {:?}",
            case
        );

        // check if total shares are updated
        let prev_total_shares = total_shares;
        let GetTotalSharesResponse { total_shares } =
            t.contract.query(&QueryMsg::GetTotalShares {}).unwrap();

        assert_eq!(
            total_shares,
            prev_total_shares - total_exit_amount,
            "check if total shares are updated after exit: case: {:?}",
            case
        );

        // check if pool liquidity is updated
        let prev_pool_liquidity = total_pool_liquidity;
        let GetTotalPoolLiquidityResponse {
            total_pool_liquidity,
        } = t
            .contract
            .query(&QueryMsg::GetTotalPoolLiquidity {})
            .unwrap();

        // zipping pool liquidity assuming that denom ordering stays the same
        prev_pool_liquidity
            .iter()
            .zip(total_pool_liquidity)
            .for_each(|(prev, curr)| {
                let exit_amount = case
                    .exit
                    .iter()
                    .find(|coin| coin.denom == curr.denom)
                    .unwrap_or(&coin(0, curr.denom))
                    .amount;
                assert_eq!(curr.amount, prev.amount - exit_amount);
            });
    }
}

#[test]
fn test_exit_pool_greater_than_their_shares_should_fail() {
    #[derive(Debug, Clone)]
    struct Case {
        join: Vec<Coin>,
        exit: Vec<Coin>,
        other_shares: Vec<Coin>,
    }

    let cases = vec![
        Case {
            join: vec![coin(1, "denoma")],
            exit: vec![coin(2, "denoma")],
            other_shares: vec![coin(1000, "denoma")],
        },
        Case {
            join: vec![coin(100_000_000, "denoma")],
            exit: vec![coin(100_000_001, "denoma")],
            other_shares: vec![coin(1000, "denoma")],
        },
        Case {
            join: vec![coin(u128::MAX - 1, "denoma")],
            exit: vec![coin(u128::MAX, "denoma")],
            other_shares: vec![coin(1, "denoma")],
        },
    ];

    for case in cases {
        let app = OsmosisTestApp::new();

        // create required denoms if not part of join or other shares
        let denoms = vec![case.join.clone(), case.other_shares.clone()]
            .concat()
            .iter()
            .map(|coin| coin.denom.clone())
            .collect::<Vec<_>>();

        if !denoms.contains(&"denoma".to_string()) {
            app.init_account(&[coin(1, "denoma")]).unwrap();
        }
        if !denoms.contains(&"denomb".to_string()) {
            app.init_account(&[coin(1, "denomb")]).unwrap();
        }

        let t = TestEnvBuilder::new()
            .with_account("addr", case.join.clone())
            .with_account("other", case.other_shares.clone())
            .with_instantiate_msg(InstantiateMsg {
                pool_asset_configs: vec![
                    AssetConfig::from_denom_str("denoma"),
                    AssetConfig::from_denom_str("denomb"),
                ],
                alloyed_asset_subdenom: "all".to_string(),
                alloyed_asset_normalization_factor: Uint128::one(),
                admin: None,
                moderator: "osmo1cyyzpxplxdzkeea7kwsydadg87357qnahakaks".to_string(),
            })
            .build(&app);

        // let other join pool
        if !case.other_shares.is_empty() {
            t.contract
                .execute(
                    &ExecMsg::JoinPool {},
                    &case.other_shares,
                    &t.accounts["other"],
                )
                .unwrap();
        }

        t.contract
            .execute(&ExecMsg::JoinPool {}, &case.join, &t.accounts["addr"])
            .unwrap();

        let err = t
            .contract
            .execute(
                &ExecMsg::ExitPool {
                    tokens_out: case.exit.clone(),
                },
                &[],
                &t.accounts["addr"],
            )
            .unwrap_err();

        assert_contract_err(
            ContractError::InsufficientShares {
                required: case
                    .exit
                    .iter()
                    .fold(Uint128::zero(), |acc, coin| acc + coin.amount),
                available: t
                    .contract
                    .query::<GetSharesResponse>(&QueryMsg::GetShares {
                        address: t.accounts["addr"].address(),
                    })
                    .unwrap()
                    .shares,
            },
            err,
        );
    }
}

#[test]
fn test_exit_pool_within_shares_but_over_joined_denom_amount() {
    let app = OsmosisTestApp::new();
    let t = TestEnvBuilder::new()
        .with_account("instantiator", vec![coin(100_000_000, "denoma")])
        .with_account("addr1", vec![coin(200_000_000, "denomb")])
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_configs: vec![
                AssetConfig::from_denom_str("denoma"),
                AssetConfig::from_denom_str("denomb"),
            ],
            alloyed_asset_subdenom: "all".to_string(),
            alloyed_asset_normalization_factor: Uint128::one(),
            admin: None,
            moderator: "osmo1cyyzpxplxdzkeea7kwsydadg87357qnahakaks".to_string(),
        })
        .build(&app);

    t.contract
        .execute(
            &ExecMsg::JoinPool {},
            &[coin(100_000_000, "denoma")],
            &t.accounts["instantiator"],
        )
        .unwrap();

    t.contract
        .execute(
            &ExecMsg::JoinPool {},
            &[coin(200_000_000, "denomb")],
            &t.accounts["addr1"],
        )
        .unwrap();

    t.contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: vec![coin(100_000_000, "denoma"), coin(100_000_000, "denomb")],
            },
            &[],
            &t.accounts["addr1"],
        )
        .expect("exit pool should succeed");
}
