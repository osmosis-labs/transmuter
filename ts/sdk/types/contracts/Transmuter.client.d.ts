/**
* This file was automatically generated by @cosmwasm/ts-codegen@0.24.0.
* DO NOT MODIFY IT BY HAND. Instead, modify the source JSONSchema file,
* and run the @cosmwasm/ts-codegen generate command to regenerate this file.
*/
import { CosmWasmClient, SigningCosmWasmClient, ExecuteResult } from "@cosmjs/cosmwasm-stargate";
import { StdFee } from "@cosmjs/amino";
import { Coin, PoolResponse, SharesResponse } from "./Transmuter.types";
export interface TransmuterReadOnlyInterface {
    contractAddress: string;
    pool: () => Promise<PoolResponse>;
    shares: ({ address }: {
        address: string;
    }) => Promise<SharesResponse>;
}
export declare class TransmuterQueryClient implements TransmuterReadOnlyInterface {
    client: CosmWasmClient;
    contractAddress: string;
    constructor(client: CosmWasmClient, contractAddress: string);
    pool: () => Promise<PoolResponse>;
    shares: ({ address }: {
        address: string;
    }) => Promise<SharesResponse>;
}
export interface TransmuterInterface extends TransmuterReadOnlyInterface {
    contractAddress: string;
    sender: string;
    joinPool: (fee?: number | StdFee | "auto", memo?: string, funds?: Coin[]) => Promise<ExecuteResult>;
    transmute: ({ tokenOutDenom }: {
        tokenOutDenom: string;
    }, fee?: number | StdFee | "auto", memo?: string, funds?: Coin[]) => Promise<ExecuteResult>;
    exitPool: ({ tokensOut }: {
        tokensOut: Coin[];
    }, fee?: number | StdFee | "auto", memo?: string, funds?: Coin[]) => Promise<ExecuteResult>;
}
export declare class TransmuterClient extends TransmuterQueryClient implements TransmuterInterface {
    client: SigningCosmWasmClient;
    sender: string;
    contractAddress: string;
    constructor(client: SigningCosmWasmClient, sender: string, contractAddress: string);
    joinPool: (fee?: number | StdFee | "auto", memo?: string, funds?: Coin[]) => Promise<ExecuteResult>;
    transmute: ({ tokenOutDenom }: {
        tokenOutDenom: string;
    }, fee?: number | StdFee | "auto", memo?: string, funds?: Coin[]) => Promise<ExecuteResult>;
    exitPool: ({ tokensOut }: {
        tokensOut: Coin[];
    }, fee?: number | StdFee | "auto", memo?: string, funds?: Coin[]) => Promise<ExecuteResult>;
}
//# sourceMappingURL=Transmuter.client.d.ts.map