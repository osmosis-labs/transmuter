use cosmwasm_std::{coin, ensure, Addr, Coin, Deps, DepsMut, Env, Response, Uint128};
use osmosis_std::types::osmosis::tokenfactory::v1beta1::MsgMint;

use crate::{
    alloyed_asset::swap_to_alloyed,
    contract::Transmuter,
    swap::{
        common::{
            set_data_if_sudo, Adjustment, Entrypoint, SwapExactAmountInResponseData,
            SwapExactAmountOutResponseData,
        },
        rebalancing_adjustment_for_exact_in, rebalancing_adjustment_for_exact_out,
    },
    transmuter_pool::TransmuterPool,
    ContractError,
};

#[derive(Debug)]
pub enum SwapToAlloyedConstraint<'a> {
    ExactIn {
        tokens_in: &'a [Coin],
        token_out_min_amount: Uint128,
    },
    ExactOut {
        token_in_denom: &'a str,
        token_in_max_amount: Uint128,
        token_out_amount: Uint128,
    },
}

impl Transmuter {
    /// Swap tokens to alloyed asset. (eg. swap nBTC -> allBTC or join pool pool nBTC, wBTC -> allBTC)
    ///
    /// Send token to the contract and mint equal value of alloyed asset to the sender.
    pub fn swap_tokens_to_alloyed_asset(
        &self,
        entrypoint: Entrypoint,
        constraint: SwapToAlloyedConstraint,
        sender: Addr,
        mut deps: DepsMut,
        env: Env,
    ) -> Result<Response, ContractError> {
        let alloyed_denom = self.alloyed_asset.get_alloyed_denom(deps.storage)?;
        let alloyed_norm_factor = self.alloyed_asset.get_normalization_factor(deps.storage)?;

        let (pool, tokens_in, out_amount, response) = match constraint {
            SwapToAlloyedConstraint::ExactIn {
                tokens_in,
                token_out_min_amount,
            } => self.swap_tokens_to_alloyed_asset_exact_in(
                entrypoint,
                tokens_in.to_owned(),
                token_out_min_amount,
                alloyed_denom.clone(),
                alloyed_norm_factor,
                deps.branch(),
                &env,
            )?,

            SwapToAlloyedConstraint::ExactOut {
                token_in_denom,
                token_in_max_amount,
                token_out_amount,
            } => self.swap_tokens_to_alloyed_asset_exact_out(
                entrypoint,
                token_in_denom,
                token_in_max_amount,
                token_out_amount,
                deps.branch(),
            )?,
        };

        // ensure funds not empty
        ensure!(
            !tokens_in.is_empty(),
            ContractError::AtLeastSingleTokenExpected {}
        );

        // ensure funds does not have zero coin
        ensure!(
            tokens_in.iter().all(|coin| coin.amount > Uint128::zero()),
            ContractError::ZeroValueOperation {}
        );

        // no need for cleaning up drained corrupted assets here
        // since this function will only adding more underlying assets
        // rather than removing any of them

        self.pool.save(deps.storage, &pool)?;

        let alloyed_asset_mint_msg = create_alloyed_asset_mint_msg(
            out_amount,
            alloyed_denom,
            &sender,
            &env.contract.address,
        );

        Ok(response.add_message(alloyed_asset_mint_msg))
    }

    fn swap_tokens_to_alloyed_asset_exact_in(
        &self,
        entrypoint: Entrypoint,
        tokens_in: Vec<Coin>,
        token_out_min_amount: Uint128,
        alloyed_denom: String,
        alloyed_norm_factor: Uint128,
        mut deps: DepsMut,
        env: &Env,
    ) -> Result<(TransmuterPool, Vec<Coin>, Uint128, Response), ContractError> {
        let pool: TransmuterPool = self.pool.load(deps.storage)?;
        let response = Response::new();
        let alloyed_incentive_pool_balance_before = self
            .incentive_pool
            .get_pool_balance(deps.storage, &alloyed_denom)?;

        let std_norm_factor = pool.std_norm_factor()?;
        let token_out_norm_factor = alloyed_norm_factor;

        let tokens_in_with_norm_factor = pool.pair_coins_with_normalization_factor(&tokens_in)?;
        let out_amount_before_fee = swap_to_alloyed::out_amount_via_exact_in(
            tokens_in_with_norm_factor,
            token_out_min_amount,
            self.alloyed_asset.get_normalization_factor(deps.storage)?,
        )?;

        let run_pool = |_deps: Deps, mut pool: TransmuterPool| {
            pool.join_pool(&tokens_in)?;
            let token_out = coin(out_amount_before_fee.u128(), alloyed_denom.clone());

            Ok((pool, token_out))
        };

        let rebalancing_adjustment = rebalancing_adjustment_for_exact_in(
            token_out_min_amount,
            std_norm_factor,
            token_out_norm_factor,
        );

        let (pool, token_out, adjustment) =
            self.rebalancer_pass(deps.branch(), pool, run_pool, rebalancing_adjustment)?;

        let response = set_data_if_sudo(
            response,
            &entrypoint,
            &SwapExactAmountInResponseData {
                token_out_amount: token_out.amount,
            },
        )?;

        let response = match adjustment {
            // fee is deducted from the minting token out, mint directly to the contract as it's already recorded as such in the incentive pool
            Adjustment::DeductFee { fee } => response.add_message(MsgMint {
                sender: env.contract.address.to_string(),
                amount: Some(fee.into()),
                mint_to_address: env.contract.address.to_string(),
            }),
            // if there is an internal swap and the incetive is in alloyed asset, mint the difference to the contract.
            Adjustment::Incentivize { .. } => {
                let incentive_pool_balance_after = self
                    .incentive_pool
                    .get_pool_balance(deps.storage, &alloyed_denom)?;

                let diff = incentive_pool_balance_after
                    .saturating_sub(alloyed_incentive_pool_balance_before);

                if diff > Uint128::zero() {
                    response.add_message(MsgMint {
                        sender: env.contract.address.to_string(),
                        amount: Some(coin(diff.u128(), alloyed_denom).into()),
                        mint_to_address: env.contract.address.to_string(),
                    })
                } else {
                    response
                }
            }
            Adjustment::None => response,
        };

        Ok((pool, tokens_in, token_out.amount, response))
    }

    fn swap_tokens_to_alloyed_asset_exact_out(
        &self,
        entrypoint: Entrypoint,
        token_in_denom: &str,
        token_in_max_amount: Uint128,
        token_out_amount: Uint128,
        mut deps: DepsMut,
    ) -> Result<(TransmuterPool, Vec<Coin>, Uint128, Response), ContractError> {
        let pool: TransmuterPool = self.pool.load(deps.storage)?;
        let response = Response::new();

        let std_norm_factor = pool.std_norm_factor()?;
        let token_in_norm_factor = pool
            .get_pool_asset_by_denom(token_in_denom)?
            .normalization_factor();
        let in_amount_before_fee = swap_to_alloyed::in_amount_via_exact_out(
            token_in_norm_factor,
            token_in_max_amount,
            token_out_amount,
            self.alloyed_asset.get_normalization_factor(deps.storage)?,
        )?;
        let token_in = coin(in_amount_before_fee.u128(), token_in_denom);

        let run_pool = |_deps: Deps, mut pool: TransmuterPool| {
            pool.join_pool(&[token_in.clone()])?;
            Ok((pool, token_in))
        };

        let rebalancing_adjustment = rebalancing_adjustment_for_exact_out(
            token_in_max_amount,
            std_norm_factor,
            token_in_norm_factor,
        );

        let (pool, token_in, _adjustment) =
            self.rebalancer_pass(deps.branch(), pool, run_pool, rebalancing_adjustment)?;

        // Unlike exact in case where token out is alloyed, there is no separate mint target required here
        // Because fee is collected from the token_in
        let response = set_data_if_sudo(
            response,
            &entrypoint,
            &SwapExactAmountOutResponseData {
                token_in_amount: token_in.amount,
            },
        )?;

        Ok((pool, vec![token_in], token_out_amount, response))
    }
}

/// Create MsgMint for minting alloyed asset.
fn create_alloyed_asset_mint_msg(
    out_amount: Uint128,
    alloyed_denom: String,
    mint_to_address: &Addr,
    contract_address: &Addr,
) -> MsgMint {
    let alloyed_asset_out = coin(out_amount.u128(), alloyed_denom);

    MsgMint {
        sender: contract_address.to_string(),
        amount: Some(alloyed_asset_out.into()),
        mint_to_address: mint_to_address.to_string(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(deprecated)]

    use super::*;
    use cosmwasm_std::{
        coin,
        testing::{mock_env, MOCK_CONTRACT_ADDR},
        to_json_binary, Addr, Coins, CosmosMsg,
    };
    use osmosis_std::types::osmosis::tokenfactory::v1beta1::MsgMint;
    use osmosis_test_tube::cosmrs::proto::prost::Message as _;
    use rstest::rstest;

    use crate::{
        asset::Asset,
        contract::Transmuter,
        swap::{
            common::test_utils::setup_fee_deduction_test,
            common::{Entrypoint, SwapExactAmountInResponseData, SwapExactAmountOutResponseData},
        },
    };

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
        use std::collections::BTreeMap;

        use cosmwasm_std::testing::{mock_dependencies, mock_env};

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
                amount: Some(coin((amount_out_before_fee + fee).u128(), "alloyed").into()),
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

        incentive_pool_balances
            .sub(coin(fee.u128(), "alloyed"))
            .unwrap();

        // check incentive pool state
        let updated_incentive_pool_balances: Coins = transmuter
            .incentive_pool
            .get_all_pool_balances(&deps.storage)
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(incentive_pool_balances, updated_incentive_pool_balances);
    }
}
