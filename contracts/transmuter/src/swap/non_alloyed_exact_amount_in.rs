use std::cmp::Ordering;

use cosmwasm_std::{
    coin, ensure, to_json_binary, Addr, BankMsg, Coin, Deps, DepsMut, Int256, Response, Uint128,
};

use crate::{
    asset::convert_amount,
    contract::Transmuter,
    swap::{Adjustment, SwapExactAmountInResponseData},
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

        let run_pool = |deps: Deps, pool: TransmuterPool| {
            self.out_amt_given_in(deps, pool, token_in, token_out_denom)
        };

        let rebalancing_adjustment =
            |pool: TransmuterPool, token_out: Coin, total_adjustment_value: Int256| {
                let std_norm_factor = pool.std_norm_factor()?;
                let token_out_norm_factor = pool
                    .get_pool_asset_by_denom(token_out_denom)?
                    .normalization_factor();

                let (token_out, adjustment) = match total_adjustment_value.cmp(&Int256::zero()) {
                    Ordering::Less => {
                        // deduct fee from token_out
                        let fee_amount = convert_amount(
                            total_adjustment_value.abs().try_into()?,
                            std_norm_factor,
                            token_out_norm_factor,
                            // rounding up means slightly more fee than required, keeps the incentive pool healthy
                            &crate::asset::Rounding::Up,
                        )?;
                        let token_out_amount: Uint128 = token_out.amount.checked_sub(fee_amount)?;
                        let token_out = coin(token_out_amount.u128(), token_out.denom.clone());
                        let token_out_denom = token_out.denom.clone();

                        (
                            token_out,
                            Adjustment::DeductFee {
                                fee: coin(fee_amount.u128(), token_out_denom),
                            },
                        )
                    }
                    Ordering::Greater => (
                        token_out,
                        Adjustment::CreditIncentive {
                            incentive: total_adjustment_value.abs().try_into()?,
                        },
                    ),
                    Ordering::Equal => (token_out, Adjustment::None),
                };

                ensure!(
                    token_out.amount >= token_out_min_amount,
                    ContractError::InsufficientTokenOut {
                        min_required: token_out_min_amount,
                        amount_out: token_out.amount,
                    }
                );

                Ok((token_out, adjustment))
            };

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
