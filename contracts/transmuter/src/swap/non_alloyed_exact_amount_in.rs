use cosmwasm_std::{to_json_binary, Addr, BankMsg, Coin, Deps, DepsMut, Response, Uint128};

use crate::{
    contract::Transmuter,
    swap::{common::SwapExactAmountInResponseData, rebalancing_adjustment_for_exact_in},
    transmuter_pool::TransmuterPool,
    ContractError,
};
impl Transmuter {
    pub fn swap_non_alloyed_exact_amount_in(
        &self,
        token_in: Coin,
        token_out_denom: &str,
        token_out_min_amount: Uint128,
        sender: Addr,
        mut deps: DepsMut,
    ) -> Result<Response, ContractError> {
        let pool = self.pool.load(deps.storage)?;
        let std_norm_factor = pool.std_norm_factor()?;
        let token_out_norm_factor = pool
            .get_pool_asset_by_denom(token_out_denom)?
            .normalization_factor();

        let run_pool = |deps: Deps, pool: TransmuterPool| {
            self.out_amt_given_in(deps, pool, token_in, token_out_denom)
        };

        let rebalancing_adjustment = rebalancing_adjustment_for_exact_in(
            token_out_min_amount,
            std_norm_factor,
            token_out_norm_factor,
        );

        let (mut pool, actual_token_out, _adjustment) = self.rebalancer_pass(
            deps.branch(),
            pool,
            &sender,
            run_pool,
            rebalancing_adjustment,
        )?;

        self.clean_up_drained_corrupted_assets(deps.storage, &mut pool)?;

        // save pool
        self.pool.save(deps.storage, &pool)?;

        let send_token_out_to_sender_msg = BankMsg::Send {
            to_address: sender.to_string(),
            amount: vec![actual_token_out.clone()],
        };

        let swap_result = SwapExactAmountInResponseData {
            token_out_amount: actual_token_out.amount,
        };

        Ok(Response::new()
            .add_message(send_token_out_to_sender_msg)
            .set_data(to_json_binary(&swap_result)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        asset::Asset,
        contract::Transmuter,
        scope::Scope,
        swap::{
            common::{test_utils::setup_fee_deduction_test, SwapExactAmountInResponseData},
            SwapExactAmountOutResponseData,
        },
    };
    use cosmwasm_std::{coin, from_json, testing::mock_dependencies, Coins, Decimal};
    use itertools::Itertools;
    use rstest::rstest;
    use std::collections::BTreeMap;
    use transmuter_math::rebalancing::config::RebalancingConfig;

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

        let updated_incentive_pool_balances = transmuter
            .incentive_pool
            .get_all_pool_balances(&deps.storage)
            .unwrap()
            .try_into()
            .unwrap();

        assert_eq!(incentive_pool_balances, updated_incentive_pool_balances);
    }
}
