# Transmuter

![CI](https://github.com/osmosis-labs/transmuter/actions/workflows/rust.yml/badge.svg)

A CosmWasm contract for 1:1 swap between two tokens with no fees.

## Overview

`transmuter` is designed to be used as [`cosmwasmpool`](https://github.com/osmosis-labs/osmosis/tree/main/x/cosmwasmpool), which is a module that allows users to create a pool of CosmWasm contracts that can be used to swap tokens.

Since the contract is designed to be used as a `cosmwasmpool`, the code needs to be upload through [`UploadCosmWasmPoolCodeAndWhiteListProposal`](https://github.com/osmosis-labs/osmosis/blob/b94dbe643ff37d93a639f49ee9b81208dbc3ba8c/proto/osmosis/cosmwasmpool/v1beta1/gov.proto#L8-L21) or [`MigratePoolContractsProposal`](https://github.com/osmosis-labs/osmosis/blob/b94dbe643ff37d93a639f49ee9b81208dbc3ba8c/proto/osmosis/cosmwasmpool/v1beta1/gov.proto#L23-L74) in case of migration instead of the usual `StoreCodeProposal`.

To crate a pool, it requires sending [`MsgCreateCosmWasmPool`](https://github.com/osmosis-labs/osmosis/blob/b94dbe643ff37d93a639f49ee9b81208dbc3ba8c/proto/osmosis/cosmwasmpool/v1beta1/model/tx.proto#L14-L20) which includes `code_id` and `instantiation_msg` for the contract, `cosmwasmpool` module will instantiate the contract under the hood and register the pool.

## Instantiate msg

```rs
struct InstantiateMsg {
    /// The denom of pool assets that can be swapped.
    pub pool_asset_denoms: Vec<String>,

    /// Admin of the contract.
    pub admin: Option<String>,
}
```

## Join and Exit pool

To join the pool, user needs to the execute the contract with the following message:

```json
{ "join_pool": {} }
```

And attach funds along with the message with the denom that is registered in the pool.

To exit the pool, user needs to the execute the contract with the following message:

```json
{
    "exit_pool": {
    "tokens_out": [
      { "denom": "uaaa", "amount": "1000000" },
      { "denom": "ubbb", "amount": "1000000" }
    ]
  }
}
```

## Swap

The swap can be performed through [`poolmanager`'s msgs](https://github.com/osmosis-labs/osmosis/tree/main/x/poolmanager#swaps) which will get routed to the contract's sudo entrypoint.

## Administration

As for now, `cosmwasmpool` module still haven't route pool deactivation to the contract. So for now the only way to deactivate the pool is to have an admin who can deactivate the pool.

Admin is set on instantiate and can send the following msg:

```json
{ "set_active_status": true }
```

Admin address can be set on instantiation of the contract. The admin can be changed by sending:

```json
{ "transfer_admin": { "candidate": "osmo1..." } }
```

Only candidate can claim the admin role by sending:

```json
{ "claim_admin": {} }
```
