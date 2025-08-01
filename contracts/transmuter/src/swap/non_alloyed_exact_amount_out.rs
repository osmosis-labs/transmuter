use std::cmp::Ordering;

use cosmwasm_std::{
    coin, ensure, to_json_binary, Addr, BankMsg, Coin, Deps, DepsMut, Int256, Response, Uint128,
};

use crate::{
    asset::convert_amount,
    contract::Transmuter,
    swap::common::{Adjustment, SwapExactAmountOutResponseData},
    transmuter_pool::TransmuterPool,
    ContractError,
};

impl Transmuter {
    pub fn swap_non_alloyed_exact_amount_out(
        &self,
        token_in_denom: &str,
        token_in_max_amount: Uint128,
        token_out: Coin,
        sender: Addr,
        mut deps: DepsMut,
    ) -> Result<Response, ContractError> {
        let pool = self.pool.load(deps.storage)?;

        let run_pool = |deps: Deps, pool: TransmuterPool| {
            self.in_amt_given_out(deps, pool, token_out.clone(), token_in_denom.to_string())
        };

        let rebalancing_adjustment =
            |pool: TransmuterPool, token_in: Coin, total_adjustment_value: Int256| {
                let std_norm_factor = pool.std_norm_factor()?;
                let token_in_norm_factor = pool
                    .get_pool_asset_by_denom(&token_in_denom)?
                    .normalization_factor();

                // If adjustment value is negative, fee take from the token_in, so we require addtional token_in // to pay for the fee.
                // Otherwise, return the token_in as is
                let (token_in, adjustment) = match total_adjustment_value.cmp(&Int256::zero()) {
                    // negative adjustment value means fee deduction from token_in
                    Ordering::Less => {
                        let fee = convert_amount(
                            total_adjustment_value.abs().try_into()?,
                            std_norm_factor,
                            token_in_norm_factor,
                            // rounding up means slightly more fee than required, keeps the incentive pool healthy
                            &crate::asset::Rounding::Up,
                        )?;

                        let token_in_amount = token_in.amount.checked_add(fee)?;

                        (
                            coin(token_in_amount.u128(), token_in.denom.clone()),
                            Adjustment::DeductFee {
                                fee: coin(fee.u128(), token_in.denom.clone()),
                            },
                        )
                    }
                    // positive adjustment value means incentive credit to the beneficiary
                    Ordering::Greater => (
                        token_in,
                        Adjustment::CreditIncentive {
                            incentive: total_adjustment_value.abs().try_into()?,
                        },
                    ),
                    // zero adjustment means no adjustment
                    Ordering::Equal => (token_in, Adjustment::None),
                };

                let token_in_amount = token_in.amount.clone();

                ensure!(
                    token_in_amount <= token_in_max_amount,
                    ContractError::ExcessiveRequiredTokenIn {
                        limit: token_in_max_amount,
                        required: token_in_amount,
                    }
                );

                Ok((token_in, adjustment))
            };

        let (mut pool, actual_token_in, _adjustment) = self.rebalancer_pass(
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
            amount: vec![token_out],
        };

        let swap_result = SwapExactAmountOutResponseData {
            token_in_amount: actual_token_in.amount,
        };

        Ok(Response::new()
            .add_message(send_token_out_to_sender_msg)
            .set_data(to_json_binary(&swap_result)?))
    }
}
