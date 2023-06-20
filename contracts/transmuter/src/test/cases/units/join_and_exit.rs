use std::collections::HashMap;

use cosmwasm_std::{attr, Coin, Uint128};
use osmosis_test_tube::{Account, OsmosisTestApp};

use crate::{
    contract::{
        ExecMsg, GetShareDenomResponse, GetSharesResponse, GetTotalPoolLiquidityResponse,
        GetTotalSharesResponse, InstantiateMsg, QueryMsg,
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
            funds: vec![Coin::new(1, "denoma")],
        },
        Case {
            funds: vec![Coin::new(1, "denomb")],
        },
        Case {
            funds: vec![Coin::new(100, "denoma")],
        },
        Case {
            funds: vec![Coin::new(100, "denomb")],
        },
        Case {
            funds: vec![Coin::new(100_000_000_000_000, "denoma")],
        },
        Case {
            funds: vec![Coin::new(100_000_000_000_000, "denomb")],
        },
        Case {
            funds: vec![Coin::new(u128::MAX, "denoma")],
        },
        Case {
            funds: vec![Coin::new(u128::MAX, "denomb")],
        },
        Case {
            funds: vec![
                Coin::new(999_999_999, "denoma"),
                Coin::new(999_999_999, "denomb"),
            ],
        },
        Case {
            funds: vec![
                Coin::new(12_000_000_000, "denoma"),
                Coin::new(999_999_999, "denomb"),
            ],
        },
    ];

    for case in cases {
        let app = OsmosisTestApp::new();
        let t = TestEnvBuilder::new()
            .with_account("provider", case.funds.clone())
            .with_instantiate_msg(crate::contract::InstantiateMsg {
                pool_asset_denoms: vec!["denoma".to_string(), "denomb".to_string()],
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
                ("addr1", vec![Coin::new(1, "denoma")]),
                ("addr2", vec![Coin::new(1, "denomb")]),
            ],
        },
        Case {
            joins: vec![("addr1", vec![Coin::new(u128::MAX, "denoma")])],
        },
        Case {
            joins: vec![("addr1", vec![Coin::new(u128::MAX, "denomb")])],
        },
        Case {
            joins: vec![
                ("addr1", vec![Coin::new(100_000, "denoma")]),
                ("addr2", vec![Coin::new(999_999_999, "denomb")]),
                ("addr3", vec![Coin::new(1, "denoma")]),
                ("addr4", vec![Coin::new(2, "denomb")]),
            ],
        },
        Case {
            joins: vec![
                (
                    "addr1",
                    vec![Coin::new(100_000, "denoma"), Coin::new(999, "denomb")],
                ),
                (
                    "addr2",
                    vec![
                        Coin::new(999_999_999, "denoma"),
                        Coin::new(999_999_999, "denomb"),
                    ],
                ),
                (
                    "addr3",
                    vec![Coin::new(1, "denoma"), Coin::new(1, "denomb")],
                ),
            ],
        },
    ];

    for case in cases {
        let app = OsmosisTestApp::new();
        let mut builder = TestEnvBuilder::new();

        for (acc, funds) in case.joins.clone() {
            builder = builder.with_account(acc, funds);
        }

        let t = builder
            .with_instantiate_msg(InstantiateMsg {
                pool_asset_denoms: vec!["denoma".to_string(), "denomb".to_string()],
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
            join: vec![Coin::new(1, "denoma")],
            exit: vec![Coin::new(1, "denoma")],
        },
        Case {
            join: vec![Coin::new(100_000, "denoma"), Coin::new(1, "denomb")],
            exit: vec![Coin::new(100_000, "denoma")],
        },
        Case {
            join: vec![Coin::new(1, "denoma"), Coin::new(100_000, "denomb")],
            exit: vec![Coin::new(100_000, "denomb")],
        },
        Case {
            join: vec![Coin::new(u128::MAX, "denoma")],
            exit: vec![Coin::new(u128::MAX, "denoma")],
        },
        Case {
            join: vec![Coin::new(u128::MAX, "denoma")],
            exit: vec![Coin::new(u128::MAX - 1, "denoma")],
        },
        Case {
            join: vec![Coin::new(u128::MAX, "denoma")],
            exit: vec![Coin::new(u128::MAX - 100_000_000, "denoma")],
        },
    ];

    for case in cases {
        // let transmuter = Transmuter::new();
        // let mut deps = mock_dependencies();
        let app = OsmosisTestApp::new();
        let t = TestEnvBuilder::new()
            .with_account("instantiator", vec![])
            .with_account("addr1", case.join.clone())
            .with_instantiate_msg(InstantiateMsg {
                pool_asset_denoms: vec!["denoma".to_string(), "denomb".to_string()],
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
                .cloned()
                .filter(|coin| !coin.amount.is_zero())
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

        // dbg!(res);

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
                attr("burn_from_address", t.contract.contract_addr.clone()),
                attr("amount", format!("{}{}", exit_amount, share_denom)),
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
                    .unwrap_or(&Coin::new(0, curr.denom))
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
    }

    let cases = vec![
        Case {
            join: vec![Coin::new(1, "denoma")],
            exit: vec![Coin::new(2, "denoma")],
        },
        Case {
            join: vec![Coin::new(100_000_000, "denoma")],
            exit: vec![Coin::new(100_000_001, "denoma")],
        },
        Case {
            join: vec![Coin::new(u128::MAX - 1, "denoma")],
            exit: vec![Coin::new(u128::MAX, "denoma")],
        },
        Case {
            join: vec![
                Coin::new(u128::MAX - 100_000_000, "denoma"),
                Coin::new(99_999_999, "denomb"),
            ],
            exit: vec![Coin::new(u128::MAX, "denoma")],
        },
    ];

    for case in cases {
        let app = OsmosisTestApp::new();
        let t = TestEnvBuilder::new()
            .with_account("addr", case.join.clone())
            .with_instantiate_msg(InstantiateMsg {
                pool_asset_denoms: vec!["denoma".to_string(), "denomb".to_string()],
            })
            .build(&app);

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
        .with_account("instantiator", vec![Coin::new(100_000_000, "denoma")])
        .with_account("addr1", vec![Coin::new(200_000_000, "denomb")])
        .with_instantiate_msg(InstantiateMsg {
            pool_asset_denoms: vec!["denoma".to_string(), "denomb".to_string()],
        })
        .build(&app);

    t.contract
        .execute(
            &ExecMsg::JoinPool {},
            &[Coin::new(100_000_000, "denoma")],
            &t.accounts["instantiator"],
        )
        .unwrap();

    t.contract
        .execute(
            &ExecMsg::JoinPool {},
            &[Coin::new(200_000_000, "denomb")],
            &t.accounts["addr1"],
        )
        .unwrap();

    t.contract
        .execute(
            &ExecMsg::ExitPool {
                tokens_out: vec![
                    Coin::new(100_000_000, "denoma"),
                    Coin::new(100_000_000, "denomb"),
                ],
            },
            &[],
            &t.accounts["addr1"],
        )
        .expect("exit pool should succeed");
}
