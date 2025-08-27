use cosmwasm_std::{
    coin, ensure, Addr, BankMsg, Coin, CosmosMsg, Deps, DepsMut, Env, Response, Uint128,
};
use osmosis_std::types::osmosis::tokenfactory::v1beta1::{MsgBurn, MsgMint};

use crate::{
    alloyed_asset::swap_from_alloyed,
    contract::Transmuter,
    corruptable::Corruptable as _,
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
pub enum SwapFromAlloyedConstraint<'a> {
    ExactIn {
        token_out_denom: &'a str,
        token_out_min_amount: Uint128,
        token_in_amount: Uint128,
    },
    ExactOut {
        tokens_out: &'a [Coin],
        token_in_max_amount: Uint128,
    },
}

/// Determines where to burn alloyed assets from.
pub enum BurnTarget {
    /// Burn alloyed asset from the sender's account.
    /// This is used when the sender wants to exit pool
    /// forcing no funds attached in the process.
    SenderAccount,
    /// Burn alloyed assets from the sent funds.
    /// This is used when the sender wants to swap tokens for alloyed assets,
    /// since alloyed asset needs to be sent to the contract before swapping.
    SentFunds,
}

impl Transmuter {
    /// Swap alloyed asset to tokens. (eg. allBTC -> nBTC or exit pool allBTC -> nBTC, wBTC)
    ///
    /// It burns alloyed asset used for the swap then sends equal value of tokens to the sender.
    pub fn swap_alloyed_asset_to_tokens(
        &self,
        entrypoint: Entrypoint,
        constraint: SwapFromAlloyedConstraint,
        burn_target: BurnTarget,
        sender: Addr,
        mut deps: DepsMut,
        env: Env,
    ) -> Result<Response, ContractError> {
        let (mut pool, in_amount, tokens_out, adjustment, response) = match constraint {
            SwapFromAlloyedConstraint::ExactIn {
                token_out_denom,
                token_out_min_amount,
                token_in_amount,
            } => self.swap_alloyed_asset_to_tokens_exact_in(
                entrypoint,
                token_out_denom,
                token_out_min_amount,
                token_in_amount,
                deps.branch(),
                env.clone(),
            )?,
            SwapFromAlloyedConstraint::ExactOut {
                tokens_out,
                token_in_max_amount,
            } => self.swap_alloyed_asset_to_tokens_exact_out(
                entrypoint,
                tokens_out,
                token_in_max_amount,
                deps.branch(),
                env.clone(),
            )?,
        };

        // ensure tokens out has no zero value
        ensure!(
            tokens_out.iter().all(|coin| coin.amount > Uint128::zero()),
            ContractError::ZeroValueOperation {}
        );

        self.clean_up_drained_corrupted_assets(deps.storage, &mut pool)?;
        self.pool.save(deps.storage, &pool)?;

        // We need to burn alloyed asset, which is token in, as it is essentially exiting pool and burn LP token.
        let burn_alloyed_asset_and_send_fee_msgs = self
            .create_burn_alloyed_asset_msg_and_send_fee_to_the_contract(
                burn_target,
                &sender,
                constraint,
                in_amount,
                adjustment,
                deps.branch(),
                env,
            )?;

        // Send tokens out to the sender.
        let bank_send_msg = BankMsg::Send {
            to_address: sender.to_string(),
            amount: tokens_out,
        };

        Ok(response
            .add_messages(burn_alloyed_asset_and_send_fee_msgs)
            .add_message(bank_send_msg))
    }

    fn swap_alloyed_asset_to_tokens_exact_in(
        &self,
        entrypoint: Entrypoint,
        token_out_denom: &str,
        token_out_min_amount: Uint128,
        token_in_amount: Uint128,
        mut deps: DepsMut,
        env: Env,
    ) -> Result<(TransmuterPool, Uint128, Vec<Coin>, Adjustment, Response), ContractError> {
        let mut pool: TransmuterPool = self.pool.load(deps.storage)?;
        let response = Response::new();
        let std_norm_factor = pool.std_norm_factor()?;
        let alloyed_denom = self.alloyed_asset.get_alloyed_denom(deps.storage)?;
        let alloyed_incentive_pool_balance_before = self
            .incentive_pool
            .get_pool_balance(deps.storage, &alloyed_denom)?;

        let token_out_norm_factor = pool
            .get_pool_asset_by_denom(token_out_denom)?
            .normalization_factor();
        let out_amount = swap_from_alloyed::out_amount_via_exact_in(
            token_in_amount,
            self.alloyed_asset.get_normalization_factor(deps.storage)?,
            token_out_norm_factor,
        )?;

        let mut token_out = coin(out_amount.u128(), token_out_denom);
        let tokens_out = vec![token_out.clone()];
        let mut adjustment = Adjustment::None;

        // If all tokens out are corrupted assets and exit with all remaining liquidity
        // then ignore the limit and remove the corrupted assets from the pool
        if self.is_force_exit_corrupted_assets(&pool, &tokens_out) {
            pool.unchecked_exit_pool(&tokens_out)?;
        } else {
            let run_pool = |_: Deps, mut pool: TransmuterPool| {
                pool.exit_pool(&tokens_out)?;
                Ok((pool, token_out))
            };

            let rebalancing_adjustment = rebalancing_adjustment_for_exact_in(
                token_out_min_amount,
                std_norm_factor,
                token_out_norm_factor,
            );

            (pool, token_out, adjustment) =
                self.rebalancer_pass(deps.branch(), pool, run_pool, rebalancing_adjustment)?;
        }

        let alloyed_incentive_pool_balance_after = self
            .incentive_pool
            .get_pool_balance(deps.storage, &alloyed_denom)?;

        let diff = alloyed_incentive_pool_balance_before
            .saturating_sub(alloyed_incentive_pool_balance_after);

        // correct excess alloyed due to internal swap (alloyed -> other denom)
        // alloyed will only be used for internal swap, so we can safely burn it.
        // the case where the incentive itself is alloyed will not occur here because
        // it's a swap from alloyed asset to token exact in, the incentive will be paid in the out token denom.
        // TODO: This could happen in non-alloyed swap, we need to handle it.
        let response =
            if matches!(adjustment, Adjustment::Incentivize { .. }) && diff > Uint128::zero() {
                response.add_message(MsgBurn {
                    sender: env.contract.address.to_string(),
                    amount: Some(coin(diff.u128(), alloyed_denom).into()),
                    burn_from_address: env.contract.address.to_string(),
                })
            } else {
                response
            };

        let response = set_data_if_sudo(
            response,
            &entrypoint,
            &SwapExactAmountInResponseData {
                token_out_amount: token_out.amount,
            },
        )?;

        let tokens_out = vec![token_out];

        Ok((pool, token_in_amount, tokens_out, adjustment, response))
    }

    fn swap_alloyed_asset_to_tokens_exact_out(
        &self,
        entrypoint: Entrypoint,
        tokens_out: &[Coin],
        token_in_max_amount: Uint128,
        mut deps: DepsMut,
        env: Env,
    ) -> Result<(TransmuterPool, Uint128, Vec<Coin>, Adjustment, Response), ContractError> {
        let mut response = Response::new();
        let mut pool: TransmuterPool = self.pool.load(deps.storage)?;
        let tokens_out_with_norm_factor = pool.pair_coins_with_normalization_factor(tokens_out)?;

        let token_in_norm_factor = self.alloyed_asset.get_normalization_factor(deps.storage)?;
        let std_norm_factor = pool.std_norm_factor()?;
        let in_amount = swap_from_alloyed::in_amount_via_exact_out(
            token_in_norm_factor,
            tokens_out_with_norm_factor,
        )?;

        let token_in_denom = self.alloyed_asset.get_alloyed_denom(deps.storage)?;
        let mut token_in = coin(in_amount.u128(), token_in_denom.clone());
        let mut adjustment = Adjustment::None;

        // If all tokens out are corrupted assets and exit with all remaining liquidity
        // then ignore the limit and remove the corrupted assets from the pool
        if self.is_force_exit_corrupted_assets(&pool, &tokens_out) {
            pool.unchecked_exit_pool(&tokens_out)?;
        } else {
            let run_pool = |_: Deps, mut pool: TransmuterPool| {
                pool.exit_pool(&tokens_out)?;
                Ok((pool, token_in.clone()))
            };

            let rebalancing_adjustment = rebalancing_adjustment_for_exact_out(
                token_in_max_amount,
                std_norm_factor,
                token_in_norm_factor,
            );

            (pool, token_in, adjustment) =
                self.rebalancer_pass(deps.branch(), pool, run_pool, rebalancing_adjustment)?;
        }

        // burn incentive from the contract, as this is used to subsidize in amount
        if let Adjustment::Incentivize { ref incentive } = adjustment {
            response = response.add_message(MsgBurn {
                sender: env.contract.address.to_string(),
                amount: Some(incentive.clone().into()),
                burn_from_address: env.contract.address.to_string(),
            });
        }

        let response = set_data_if_sudo(
            response,
            &entrypoint,
            &SwapExactAmountOutResponseData {
                token_in_amount: token_in.amount,
            },
        )?;

        Ok((
            pool,
            token_in.amount,
            tokens_out.to_vec(),
            adjustment,
            response,
        ))
    }

    /// Create MsgBurn to burn alloyed assets from the sender or sent funds
    /// based on the burn target.
    ///
    /// If the constraint is exact out, and the adjustment is deduct fee, it means we deduct fee from in amount, which is alloyed
    /// In that case we keep the fee portion in contract and burn the rest. Incentive pool accounting is handled within [Transmuter::rebalancer_pass].
    ///
    /// Keep burn amount as is otherwise.
    ///
    /// If deduct fee, we send the fee portion to the contract in case it directly burns from the sender.
    fn create_burn_alloyed_asset_msg_and_send_fee_to_the_contract(
        &self,
        burn_target: BurnTarget,
        sender: &Addr,
        constraint: SwapFromAlloyedConstraint,
        in_amount: Uint128,
        adjustment: Adjustment,
        deps: DepsMut,
        env: Env,
    ) -> Result<Vec<CosmosMsg>, ContractError> {
        let alloyed_denom = self.alloyed_asset.get_alloyed_denom(deps.storage)?;
        let mut messages = vec![];
        let burn_from_address = match &burn_target {
            BurnTarget::SenderAccount => {
                // Check if the sender's shares is sufficient to burn
                let shares = self.alloyed_asset.get_balance(deps.as_ref(), &sender)?;
                ensure!(
                    shares >= in_amount,
                    ContractError::InsufficientShares {
                        required: in_amount,
                        available: shares
                    }
                );

                Ok::<&Addr, ContractError>(&sender)
            }

            // Burn from the sent funds, funds are guaranteed to be sent via cw-pool mechanism
            // But to defend in depth, we still check the balance of the contract.
            // Theoretically, alloyed asset balance should always remain 0 before any tx since
            // it is always received and burned or minted and sent to another address.
            // Except for the case where the contract is funded with alloyed assets directly
            // that is not as part of transmuter mechanism.
            //
            // So it's safe to check just check that contract has enough alloyed assets to burn.
            // Since it's only being a loss for the actor that does not follow the normal mechanism.
            BurnTarget::SentFunds => {
                // get alloyed denom contract balance
                let alloyed_contract_balance = self
                    .alloyed_asset
                    .get_balance(deps.as_ref(), &env.contract.address)?;

                // ensure that alloyed contract balance is greater than in_amount
                ensure!(
                    alloyed_contract_balance >= in_amount,
                    ContractError::InsufficientShares {
                        required: in_amount,
                        available: alloyed_contract_balance
                    }
                );

                Ok(&env.contract.address)
            }
        }?
        .to_string();

        let burn_amount = match (constraint, adjustment) {
            // If the constraint is exact out, and the adjustment is deduct fee, it means we deduct fee from in amount, which is alloyed
            // In that case we keep the fee portion in contract and burn the rest
            (SwapFromAlloyedConstraint::ExactOut { .. }, Adjustment::DeductFee { fee }) => {
                // if burn target is sender account, mint the fee portion to the contract
                // otherwise, deduct the fee from in amount to keep fee portion in contract
                if let BurnTarget::SenderAccount = burn_target {
                    messages.push(
                        MsgMint {
                            sender: env.contract.address.to_string(),
                            amount: Some(coin(fee.amount.u128(), &alloyed_denom).into()),
                            mint_to_address: env.contract.address.to_string(),
                        }
                        .into(),
                    );
                    in_amount
                } else {
                    in_amount.checked_sub(fee.amount)?
                }
            }
            _ => in_amount,
        };

        let alloyed_asset_to_burn = coin(burn_amount.u128(), alloyed_denom).into();

        messages.push(
            MsgBurn {
                sender: env.contract.address.to_string(),
                amount: Some(alloyed_asset_to_burn),
                burn_from_address,
            }
            .into(),
        );

        Ok(messages)
    }

    /// Check if the tokens out are all corrupted assets and the pool is empty after exiting.
    /// If so, we can force exit the pool and remove the corrupted assets from the pool.
    fn is_force_exit_corrupted_assets(&self, pool: &TransmuterPool, tokens_out: &[Coin]) -> bool {
        let denoms_in_corrupted_asset_group = pool
            .asset_groups
            .iter()
            .flat_map(|(_, asset_group)| {
                if asset_group.is_corrupted() {
                    asset_group.denoms().to_vec()
                } else {
                    vec![]
                }
            })
            .collect::<Vec<_>>();

        tokens_out.iter().all(|coin| {
            let total_liquidity = pool
                .get_pool_asset_by_denom(&coin.denom)
                .map(|asset| asset.amount())
                .unwrap_or_default();
            let is_redeeming_total_liquidity = coin.amount == total_liquidity;
            let is_under_corrupted_asset_group =
                denoms_in_corrupted_asset_group.contains(&coin.denom);

            is_redeeming_total_liquidity
                && (is_under_corrupted_asset_group || pool.is_corrupted_asset(&coin.denom))
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(deprecated)]

    use super::*;

    use cosmwasm_std::testing::mock_env;
    use cosmwasm_std::{coin, testing::MOCK_CONTRACT_ADDR, to_json_binary, Addr, Decimal};
    use cosmwasm_std::{Coins, CosmosMsg};
    use osmosis_std::types::osmosis::tokenfactory::v1beta1::{MsgBurn, MsgMint};
    use osmosis_test_tube::cosmrs::proto::prost::Message as _;
    use rstest::rstest;
    use std::collections::BTreeMap;
    use transmuter_math::rebalancing::config::RebalancingConfig;

    use crate::swap::common::test_utils::setup_fee_deduction_test;
    use crate::swap::SwapToAlloyedConstraint;
    use crate::{
        asset::Asset,
        contract::Transmuter,
        scope::Scope,
        swap::common::{Entrypoint, SwapExactAmountInResponseData, SwapExactAmountOutResponseData},
    };

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
