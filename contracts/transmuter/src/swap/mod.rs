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

    use crate::{
        asset::Asset,
        swap::common::{Entrypoint, SwapExactAmountInResponseData, SwapExactAmountOutResponseData},
        ContractError,
    };
    use crate::{contract::Transmuter, transmuter_pool::TransmuterPool};
    use cosmwasm_std::Decimal;
    use cosmwasm_std::{
        coin, from_json,
        testing::{mock_env, MockApi, MockQuerier, MockStorage, MOCK_CONTRACT_ADDR},
        Addr, BankMsg, Coins, CosmosMsg, OwnedDeps, SubMsg, Uint128,
    };

    use osmosis_std::types::osmosis::tokenfactory::v1beta1::{MsgBurn, MsgMint};
    use osmosis_test_tube::cosmrs::proto::prost::Message;
    use transmuter_math::rebalancing::config::RebalancingConfig;

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
