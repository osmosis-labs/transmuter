use cosmwasm_std::testing::{
    mock_dependencies, mock_env, mock_info, MockApi, MockQuerier, MockStorage,
};
use cosmwasm_std::{BankMsg, Coin, Empty, OwnedDeps, ReplyOn, Response, SubMsg, Uint128};

use crate::contract::Transmuter;
use crate::ContractError;

#[test]
fn test_no_assets_pass() {
    let pool_assets = &[Coin::new(0, "denom0"), Coin::new(0, "denom1")];

    for (token_in, token_out) in vec![
        (
            TokenIn::MatchedFund(Coin::new(0, "denom0")),
            Coin::new(0, "denom1"),
        ),
        (
            TokenIn::MatchedFund(Coin::new(0, "denom1")),
            Coin::new(0, "denom0"),
        ),
    ] {
        assert_ok_with_bank_send(
            prep_and_swaps_exact_amount_in(pool_assets, token_in.clone(), token_out.clone()),
            vec![token_out.clone()],
        );

        assert_ok_with_bank_send(
            prep_and_swaps_exact_amount_out(pool_assets, token_in, token_out.clone()),
            vec![token_out],
        );
    }
}

#[test]
fn test_no_assets_failed() {
    let pool_assets = &[Coin::new(0, "denom0"), Coin::new(0, "denom1")];

    struct Case {
        token_in: TokenIn,
        token_out: Coin,
        expected: Box<dyn Fn() -> ContractError>,
    }

    let cases: Vec<Case> = vec![
        // insufficient token out in pool
        Case {
            token_in: TokenIn::MatchedFund(Coin::new(0, "denom0")),
            token_out: Coin::new(1, "denom1"),
            // exact in requires min token out
            expected: Box::new(|| ContractError::InsufficientTokenOut {
                required: Uint128::one(),
                available: Uint128::zero(),
            }),
        },
        // funds greater than token in
        Case {
            token_in: TokenIn::MismatchedFund {
                fund: Coin::new(1, "denom0"),
                token_in: Coin::new(0, "denom0"),
            },
            token_out: Coin::new(0, "denom1"),
            expected: Box::new(|| ContractError::FundsMismatchTokenIn {
                funds: vec![Coin::new(1, "denom0")],
                token_in: Coin::new(0, "denom0"),
            }),
        },
        // funds less than token in
        Case {
            token_in: TokenIn::MismatchedFund {
                fund: Coin::new(0, "denom0"),
                token_in: Coin::new(1, "denom0"),
            },
            token_out: Coin::new(0, "denom1"),
            expected: Box::new(|| ContractError::FundsMismatchTokenIn {
                funds: vec![Coin::new(0, "denom0")],
                token_in: Coin::new(1, "denom0"),
            }),
        },
    ];

    // swap_exact_amount_in cases
    for Case {
        token_in,
        token_out,
        expected,
    } in cases
    {
        assert_contract_error(
            prep_and_swaps_exact_amount_in(pool_assets, token_in.clone(), token_out.clone()),
            expected(),
        );
    }

    // TODO: swap_exact_amount_out cases
}

fn assert_ok_with_bank_send(res: Result<Response, ContractError>, expected: Vec<Coin>) {
    let res = res.unwrap();
    assert_eq!(
        res.messages,
        vec![SubMsg {
            id: 0,
            msg: BankMsg::Send {
                to_address: "swapper".into(),
                amount: expected,
            }
            .into(),
            gas_limit: None,
            reply_on: ReplyOn::Never,
        }]
    );
}

fn assert_contract_error(res: Result<Response, ContractError>, expected: ContractError) {
    let err = res.unwrap_err();
    assert_eq!(err, expected);
}

#[derive(Clone)]
enum TokenIn {
    MatchedFund(Coin),
    MismatchedFund { fund: Coin, token_in: Coin },
}

impl TokenIn {
    fn funds(&self) -> Vec<Coin> {
        match self {
            TokenIn::MatchedFund(fund) => vec![fund.clone()],
            TokenIn::MismatchedFund { fund, token_in: _ } => vec![fund.clone()],
        }
    }

    fn token_in(&self) -> Coin {
        match self {
            TokenIn::MatchedFund(fund) => fund.clone(),
            TokenIn::MismatchedFund { fund: _, token_in } => token_in.clone(),
        }
    }
}

fn prep_and_swaps_exact_amount_in(
    pool_assets: &[Coin],
    token_in: TokenIn,
    token_out_min: Coin,
) -> Result<Response, ContractError> {
    let mut deps = mock_dependencies();
    let transmuter = Transmuter::new();

    prep(&transmuter, &mut deps, pool_assets).unwrap();

    transmuter.swap_exact_amount_in(
        (
            deps.as_mut(),
            mock_env(),
            mock_info("swapper", &token_in.funds()),
        ),
        token_in.token_in(),
        token_out_min.denom,
        token_out_min.amount,
    )
}
fn prep_and_swaps_exact_amount_out(
    pool_assets: &[Coin],
    token_in: TokenIn,
    token_out: Coin,
) -> Result<Response, ContractError> {
    let mut deps = mock_dependencies();
    let transmuter = Transmuter::new();

    prep(&transmuter, &mut deps, pool_assets).unwrap();

    transmuter.swap_exact_amount_out(
        (
            deps.as_mut(),
            mock_env(),
            mock_info("swapper", &token_in.funds()),
        ),
        token_in.token_in().denom,
        token_in.token_in().amount,
        token_out,
    )
}

fn prep(
    transmuter: &Transmuter,
    deps: &mut OwnedDeps<MockStorage, MockApi, MockQuerier, Empty>,
    pool_assets: &[Coin],
) -> Result<(), ContractError> {
    // instantiate contract
    transmuter.instantiate(
        (deps.as_mut(), mock_env(), mock_info("instantiator", &[])),
        pool_assets.iter().map(|c| c.denom.clone()).collect(),
    )?;

    // join pool with initial tokens
    transmuter.join_pool((deps.as_mut(), mock_env(), mock_info("joiner", pool_assets)))?;

    Ok(())
}
