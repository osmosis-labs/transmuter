/**
* This file was automatically generated by @cosmwasm/ts-codegen@0.24.0.
* DO NOT MODIFY IT BY HAND. Instead, modify the source JSONSchema file,
* and run the @cosmwasm/ts-codegen generate command to regenerate this file.
*/
export type ExecuteMsg = {
    set_active_status: {
        active: boolean;
        [k: string]: unknown;
    };
} | {
    join_pool: {
        [k: string]: unknown;
    };
} | {
    exit_pool: {
        tokens_out: Coin[];
        [k: string]: unknown;
    };
} | {
    transfer_admin: {
        candidate: string;
        [k: string]: unknown;
    };
} | {
    claim_admin: {
        [k: string]: unknown;
    };
};
export type Uint128 = string;
export interface Coin {
    amount: Uint128;
    denom: string;
    [k: string]: unknown;
}
export interface InstantiateMsg {
    admin?: string | null;
    pool_asset_denoms: string[];
    [k: string]: unknown;
}
export type QueryMsg = {
    get_shares: {
        address: string;
        [k: string]: unknown;
    };
} | {
    get_share_denom: {
        [k: string]: unknown;
    };
} | {
    get_swap_fee: {
        [k: string]: unknown;
    };
} | {
    is_active: {
        [k: string]: unknown;
    };
} | {
    get_total_shares: {
        [k: string]: unknown;
    };
} | {
    get_total_pool_liquidity: {
        [k: string]: unknown;
    };
} | {
    spot_price: {
        base_asset_denom: string;
        quote_asset_denom: string;
        [k: string]: unknown;
    };
} | {
    calc_out_amt_given_in: {
        swap_fee: Decimal;
        token_in: Coin;
        token_out_denom: string;
        [k: string]: unknown;
    };
} | {
    calc_in_amt_given_out: {
        swap_fee: Decimal;
        token_in_denom: string;
        token_out: Coin;
        [k: string]: unknown;
    };
} | {
    get_admin: {
        [k: string]: unknown;
    };
} | {
    get_admin_candidate: {
        [k: string]: unknown;
    };
};
export type Decimal = string;
export interface CalcInAmtGivenOutResponse {
    token_in: Coin;
}
export interface CalcOutAmtGivenInResponse {
    token_out: Coin;
}
export type Addr = string;
export interface GetAdminCandidateResponse {
    admin_candidate?: Addr | null;
}
export interface GetAdminResponse {
    admin: Addr;
}
export interface GetShareDenomResponse {
    share_denom: string;
}
export interface GetSharesResponse {
    shares: Uint128;
}
export interface GetSwapFeeResponse {
    swap_fee: Decimal;
}
export interface GetTotalPoolLiquidityResponse {
    total_pool_liquidity: Coin[];
}
export interface GetTotalSharesResponse {
    total_shares: Uint128;
}
export interface IsActiveResponse {
    is_active: boolean;
}
export interface SpotPriceResponse {
    spot_price: Decimal;
}
//# sourceMappingURL=Transmuter.types.d.ts.map