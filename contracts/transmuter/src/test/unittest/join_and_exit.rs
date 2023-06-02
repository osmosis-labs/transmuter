use std::collections::HashMap;

use cosmwasm_std::{
    testing::{mock_dependencies, mock_env, mock_info},
    Addr, BankMsg, Coin, SubMsg, Uint128,
};
use osmosis_std::types::osmosis::tokenfactory::v1beta1::MsgBurn;

use crate::{contract::Transmuter, ContractError};

#[test]
fn test_join_pool_with_single_lp_should_update_shares_and_liquidity_properly() {
    #[derive(Debug)]
    struct Case {
        funds: Vec<Coin>,
    }

    let cases = vec![
        Case {
            funds: vec![Coin::new(0, "denom0")],
        },
        Case {
            funds: vec![Coin::new(0, "denom1")],
        },
        Case {
            funds: vec![Coin::new(1, "denom0")],
        },
        Case {
            funds: vec![Coin::new(1, "denom1")],
        },
        Case {
            funds: vec![Coin::new(100, "denom0")],
        },
        Case {
            funds: vec![Coin::new(100, "denom1")],
        },
        Case {
            funds: vec![Coin::new(100_000_000_000_000, "denom0")],
        },
        Case {
            funds: vec![Coin::new(100_000_000_000_000, "denom1")],
        },
        Case {
            funds: vec![Coin::new(u128::MAX, "denom0")],
        },
        Case {
            funds: vec![Coin::new(u128::MAX, "denom1")],
        },
        Case {
            funds: vec![
                Coin::new(999_999_999, "denom0"),
                Coin::new(999_999_999, "denom1"),
            ],
        },
        Case {
            funds: vec![
                Coin::new(12_000_000_000, "denom0"),
                Coin::new(999_999_999, "denom1"),
            ],
        },
    ];

    for case in cases {
        let transmuter = Transmuter::new();
        let mut deps = mock_dependencies();

        transmuter
            .instantiate(
                (deps.as_mut(), mock_env(), mock_info("instantiator", &[])),
                vec!["denom0".to_string(), "denom1".to_string()],
            )
            .unwrap();

        transmuter
            .shares
            .set_share_denom(
                &mut deps.storage,
                &"factory/contract_address/transmuter/poolshare".to_string(),
            )
            .unwrap();

        transmuter
            .join_pool((deps.as_mut(), mock_env(), mock_info("addr1", &case.funds)))
            .unwrap();

        // check if shares are updated
        let shares = transmuter
            .get_shares((deps.as_ref(), mock_env()), "addr1".to_string())
            .unwrap()
            .shares;

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
        let total_shares = transmuter
            .get_total_shares((deps.as_ref(), mock_env()))
            .unwrap()
            .total_shares;

        assert_eq!(total_shares, shares);

        // check if pool liquidity is updated
        let pool_liquidity: Vec<Coin> = transmuter
            .get_total_pool_liquidity((deps.as_ref(), mock_env()))
            .unwrap()
            .total_pool_liquidity
            .into_iter()
            .filter(|coin| !coin.amount.is_zero())
            .collect();

        assert_eq!(
            pool_liquidity,
            case.funds
                .iter()
                .cloned()
                .filter(|coin| !coin.amount.is_zero())
                .collect::<Vec<Coin>>(),
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
                ("addr1", vec![Coin::new(0, "denom0")]),
                ("addr2", vec![Coin::new(0, "denom1")]),
            ],
        },
        Case {
            joins: vec![("addr1", vec![Coin::new(u128::MAX, "denom0")])],
        },
        Case {
            joins: vec![("addr1", vec![Coin::new(u128::MAX, "denom1")])],
        },
        Case {
            joins: vec![
                ("addr1", vec![Coin::new(100_000, "denom0")]),
                ("addr2", vec![Coin::new(999_999_999, "denom1")]),
                ("addr3", vec![Coin::new(1, "denom0")]),
                ("addr4", vec![Coin::new(0, "denom1")]),
            ],
        },
        Case {
            joins: vec![
                (
                    "addr1",
                    vec![Coin::new(100_000, "denom0"), Coin::new(999, "denom1")],
                ),
                (
                    "addr2",
                    vec![
                        Coin::new(999_999_999, "denom0"),
                        Coin::new(999_999_999, "denom1"),
                    ],
                ),
                (
                    "addr3",
                    vec![Coin::new(1, "denom0"), Coin::new(1, "denom1")],
                ),
                (
                    "addr4",
                    vec![Coin::new(0, "denom0"), Coin::new(0, "denom1")],
                ),
            ],
        },
    ];

    for case in cases {
        let transmuter = Transmuter::new();
        let mut deps = mock_dependencies();

        transmuter
            .instantiate(
                (deps.as_mut(), mock_env(), mock_info("instantiator", &[])),
                vec!["denom0".to_string(), "denom1".to_string()],
            )
            .unwrap();

        transmuter
            .shares
            .set_share_denom(
                &mut deps.storage,
                &"factory/contract_address/transmuter/poolshare".to_string(),
            )
            .unwrap();

        for (addr, funds) in case.joins.clone() {
            transmuter
                .join_pool((deps.as_mut(), mock_env(), mock_info(addr, &funds)))
                .unwrap();
            // check if shares are updated
            let shares = transmuter
                .get_shares((deps.as_ref(), mock_env()), addr.to_string())
                .unwrap()
                .shares;

            // shares == sum of cases.funds amount
            assert_eq!(
                shares,
                Uint128::from(
                    funds
                        .iter()
                        .fold(0, |acc, coin| { acc + coin.amount.u128() })
                ),
                "checking if shares are properly updated:\n addr: {},\n funds: {:?},\n case: {:?}",
                addr,
                funds,
                case
            );
        }

        // check if total shares are updated
        let total_shares = transmuter
            .get_total_shares((deps.as_ref(), mock_env()))
            .unwrap()
            .total_shares;

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
        let pool_liquidity: Vec<Coin> = transmuter
            .get_total_pool_liquidity((deps.as_ref(), mock_env()))
            .unwrap()
            .total_pool_liquidity
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
            pool_liquidity, expected_pool_liquidity,
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
            join: vec![Coin::new(0, "denom0")],
            exit: vec![Coin::new(0, "denom0")],
        },
        Case {
            join: vec![Coin::new(100_000, "denom0"), Coin::new(1, "denom1")],
            exit: vec![Coin::new(100_000, "denom0")],
        },
        Case {
            join: vec![Coin::new(1, "denom0"), Coin::new(100_000, "denom1")],
            exit: vec![Coin::new(100_000, "denom1")],
        },
        Case {
            join: vec![Coin::new(u128::MAX, "denom0")],
            exit: vec![Coin::new(u128::MAX, "denom0")],
        },
        Case {
            join: vec![Coin::new(u128::MAX, "denom0")],
            exit: vec![Coin::new(u128::MAX - 1, "denom0")],
        },
        Case {
            join: vec![Coin::new(u128::MAX, "denom0")],
            exit: vec![Coin::new(u128::MAX - 100_000_000, "denom0")],
        },
    ];

    for case in cases {
        let transmuter = Transmuter::new();
        let mut deps = mock_dependencies();

        transmuter
            .instantiate(
                (deps.as_mut(), mock_env(), mock_info("instantiator", &[])),
                vec!["denom0".to_string(), "denom1".to_string()],
            )
            .unwrap();

        let share_denom = "factory/contract_address/transmuter/poolshare".to_string();

        transmuter
            .shares
            .set_share_denom(&mut deps.storage, &share_denom)
            .unwrap();

        transmuter
            .join_pool((deps.as_mut(), mock_env(), mock_info("addr1", &case.join)))
            .unwrap();

        // check if shares are updated
        let shares = transmuter
            .get_shares((deps.as_ref(), mock_env()), "addr1".to_string())
            .unwrap()
            .shares;

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
        let total_shares = transmuter
            .get_total_shares((deps.as_ref(), mock_env()))
            .unwrap()
            .total_shares;

        assert_eq!(total_shares, shares);

        // check if pool liquidity is updated
        let pool_liquidity: Vec<Coin> = transmuter
            .get_total_pool_liquidity((deps.as_ref(), mock_env()))
            .unwrap()
            .total_pool_liquidity
            .into_iter()
            .filter(|coin| !coin.amount.is_zero())
            .collect();

        assert_eq!(
            pool_liquidity,
            case.join
                .iter()
                .cloned()
                .filter(|coin| !coin.amount.is_zero())
                .collect::<Vec<Coin>>(),
            "check if pool liquidity is updated: {:?}",
            case
        );

        // exit pool
        let res = transmuter
            .exit_pool(
                (deps.as_mut(), mock_env(), mock_info("addr1", &[])),
                case.exit.clone(),
            )
            .unwrap();

        // sum of exit amount
        let exit_amount = case
            .exit
            .iter()
            .fold(Uint128::zero(), |acc, coin| acc + coin.amount)
            .u128();

        assert_eq!(
            res.messages,
            vec![
                SubMsg {
                    id: 0,
                    msg: MsgBurn {
                        sender: mock_env().contract.address.to_string(),
                        amount: Some(Coin::new(exit_amount, share_denom).into()),
                        burn_from_address: "addr1".to_string(),
                    }
                    .into(),
                    gas_limit: None,
                    reply_on: cosmwasm_std::ReplyOn::Never
                },
                SubMsg {
                    id: 0,
                    msg: BankMsg::Send {
                        to_address: "addr1".to_string(),
                        amount: case.exit.clone()
                    }
                    .into(),
                    gas_limit: None,
                    reply_on: cosmwasm_std::ReplyOn::Never
                }
            ]
        );

        let total_exit_amount = case
            .exit
            .iter()
            .fold(Uint128::zero(), |acc, coin| acc + coin.amount);

        // check if shares are updated
        let prev_shares = shares;
        let shares = transmuter
            .get_shares((deps.as_ref(), mock_env()), "addr1".to_string())
            .unwrap()
            .shares;
        assert_eq!(
            shares,
            prev_shares - total_exit_amount,
            "check if shares are updated after exit: case: {:?}",
            case
        );

        // check if total shares are updated
        let prev_total_shares = total_shares;
        let total_shares = transmuter
            .get_total_shares((deps.as_ref(), mock_env()))
            .unwrap()
            .total_shares;

        assert_eq!(
            total_shares,
            prev_total_shares - total_exit_amount,
            "check if total shares are updated after exit: case: {:?}",
            case
        );

        // check if pool liquidity is updated
        let prev_pool_liquidity = pool_liquidity;
        let pool_liquidity: Vec<Coin> = transmuter
            .get_total_pool_liquidity((deps.as_ref(), mock_env()))
            .unwrap()
            .total_pool_liquidity;

        // zipping pool liquidity assuming that denom ordering stays the same
        prev_pool_liquidity
            .iter()
            .zip(pool_liquidity)
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
            join: vec![Coin::new(0, "denom0")],
            exit: vec![Coin::new(1, "denom0")],
        },
        Case {
            join: vec![Coin::new(100_000_000, "denom0")],
            exit: vec![Coin::new(100_000_001, "denom0")],
        },
        Case {
            join: vec![Coin::new(u128::MAX - 1, "denom0")],
            exit: vec![Coin::new(u128::MAX, "denom0")],
        },
        Case {
            join: vec![
                Coin::new(u128::MAX - 100_000_000, "denom0"),
                Coin::new(99_999_999, "denom1"),
            ],
            exit: vec![Coin::new(u128::MAX, "denom0")],
        },
    ];

    for case in cases {
        let transmuter = Transmuter::new();
        let mut deps = mock_dependencies();

        transmuter
            .instantiate(
                (deps.as_mut(), mock_env(), mock_info("instantiator", &[])),
                vec!["denom0".to_string(), "denom1".to_string()],
            )
            .unwrap();

        transmuter
            .shares
            .set_share_denom(
                &mut deps.storage,
                &"factory/contract_address/transmuter/poolshare".to_string(),
            )
            .unwrap();

        transmuter
            .join_pool((deps.as_mut(), mock_env(), mock_info("addr1", &case.join)))
            .unwrap();

        let err = transmuter
            .exit_pool(
                (deps.as_mut(), mock_env(), mock_info("addr1", &[])),
                case.exit.clone(),
            )
            .unwrap_err();

        assert_eq!(
            err,
            ContractError::InsufficientShares {
                required: case
                    .exit
                    .iter()
                    .fold(Uint128::zero(), |acc, coin| { acc + coin.amount }),
                available: transmuter
                    .shares
                    .get_share(deps.as_ref().storage, &Addr::unchecked("addr1"))
                    .unwrap()
            }
        )
    }
}

#[test]
fn test_exit_pool_within_shares_but_over_joined_denom_amount() {
    let transmuter = Transmuter::new();
    let mut deps = mock_dependencies();

    transmuter
        .instantiate(
            (deps.as_mut(), mock_env(), mock_info("instantiator", &[])),
            vec!["denom0".to_string(), "denom1".to_string()],
        )
        .unwrap();

    transmuter
        .shares
        .set_share_denom(
            &mut deps.storage,
            &"factory/contract_address/transmuter/poolshare".to_string(),
        )
        .unwrap();

    transmuter
        .join_pool((
            deps.as_mut(),
            mock_env(),
            mock_info("instantiator", &[Coin::new(100_000_000, "denom0")]),
        ))
        .unwrap();

    transmuter
        .join_pool((
            deps.as_mut(),
            mock_env(),
            mock_info("addr1", &[Coin::new(200_000_000, "denom1")]),
        ))
        .unwrap();

    transmuter
        .exit_pool(
            (deps.as_mut(), mock_env(), mock_info("addr1", &[])),
            vec![
                Coin::new(100_000_000, "denom0"),
                Coin::new(100_000_000, "denom1"),
            ],
        )
        .expect("exit pool should succeed");
}
