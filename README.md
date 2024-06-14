# Transmuter

![CI](https://github.com/osmosis-labs/transmuter/actions/workflows/rust.yml/badge.svg)

A CosmWasm contract for X:Y swapping between multiple tokens with no fees.

## Stored Codes

| Version | Network       | Code Id                                                             |
| ------- | ------------- | ------------------------------------------------------------------- |
| `1.0.0` | `osmo-test-5` | [`3084`](https://celatone.osmosis.zone/osmo-test-5/codes/3084/info) |
|         | `osmosis-1`   | [`148`](https://celatone.osmosis.zone/osmosis-1/codes/148/info)     |
| `2.0.0` | `osmo-test-5` | [`4643`](https://celatone.osmosis.zone/osmo-test-5/codes/4643/info) |
|         | `osmosis-1`   | [`254`](https://celatone.osmosis.zone/osmosis-1/codes/254/info)     |
| `3.0.0` | `osmo-test-5` | [`8319`](https://celatone.osmosis.zone/osmo-test-5/codes/8319)      |
|         | `osmosis-1`   | [`814`](https://celatone.osmosis.zone/osmosis-1/codes/814)          |

## Overview

`transmuter` is designed to be used as a [`cosmwasmpool`](https://github.com/osmosis-labs/osmosis/tree/main/x/cosmwasmpool) module. This module enables users to create pools of CosmWasm contracts for token swapping.

Joining or exiting the pool can also be seen as swapping between the pool token and the share token, therefor the share token is treated as a swappable asset and can be exchanged with other tokens using [`poolmanager`'s msgs](https://github.com/osmosis-labs/osmosis/tree/main/x/poolmanager#swaps). The share token is also referred to as the [Alloyed Asset](#alloyed-assets).

Since the contract is designed to be used as a `cosmwasmpool`, the code needs to be upload through [`UploadCosmWasmPoolCodeAndWhiteListProposal`](https://github.com/osmosis-labs/osmosis/blob/b94dbe643ff37d93a639f49ee9b81208dbc3ba8c/proto/osmosis/cosmwasmpool/v1beta1/gov.proto#L8-L21) or [`MigratePoolContractsProposal`](https://github.com/osmosis-labs/osmosis/blob/b94dbe643ff37d93a639f49ee9b81208dbc3ba8c/proto/osmosis/cosmwasmpool/v1beta1/gov.proto#L23-L74) in case of migration instead of the usual `StoreCodeProposal`.

## Alloyed Asset

`Alloyed Asset` is an asset that is minted when a user joins the pool and burned when a user exits the pool.

- The amount of `Alloyed Asset` minted is equal to the amount of tokens that the user has deposited.
- The amount of `Alloyed Asset` burned is equal to the amount of tokens that the user has withdrawn.
- The `Alloyed Asset` is treated as a swappable asset and can be exchanged 1:1 with other tokens.
  - `Alloyed Asset` as token out will be minted to the user.
  - `Alloyed Asset` as token in will be burned from the user.

Since the `Alloyed Asset` represents tokens that are deposited in the pool, it can be viewed as a token whose value is backed by the underlying tokens in the pool. The risk exposure from each of the underlying tokens to the `Alloyed Asset` is determined by the weight of each token in the pool. To facilitate risk management, we aim to limit changes in risk through [`Limiters`](#limiters).

## Limiters

There are 2 types of limiters:

- [Change Limiter](#change-limiter)
- [Static Limiter](#static-limiter)

Each of these limiters can be used to restrict the maximum weight of each token in the pool. This is important because tokens with higher weights pose a greater risk exposure to the `Alloyed Asset`. These limiters can be used in combination with each other.

### Change Limiter

The Change Limiter determines the upper bound limit based on the Simple Moving Average (SMA) of the pool asset's weights. The SMA is calculated using data points that are divided into divisions, which are compressed for efficient storage read and reduced gas consumption since calculating average of sliding window can require a lot of gas due to read operations.

This can be used in different timeframes to prevent both fast and slow bleeding of the pool asset's weights.

### Static Limiter

The Static Limiter determines the upper bound limit based on the pool asset's weights. This serves as limitation for worst case scenarios allowed.

## Normalization Factors

Each asset can have different weights in terms of value in the pool, so to make the value of each asset equal, the value of each asset is normalized by a factor. The factor is determined by the admin of the pool and can be changed by the admin.

For example, asset `A` and asset `B` have the same value in their display denom, but exponent of `A` is `6` and exponent of `B` is `18`.
To equalize the value of each asset, the normalization factor for `A` is calculated as `10^(18-6) = 10^12`, making the value of `A` equivalent to `B` when `B`'s normalization factor is `10^(18-18) = 10^0 = 1`.

To explain the logic behind this calculation, the formula is `normalization_factor(x) = LCM([asset_weights_per_referenced_unit]) / asset_weight_per_referenced_unit(x)`:

- `LCM(A, B) = A * B / GCD(A, B) = 10^18 * 10^6 / 10^6 = 10^18`
- `normalization_factor(A) = 10^18 / 10^6 = 10^12`
- `normalization_factor(B) = 10^18 / 10^18 = 10^0 = 1`

With this, we can compare the value of each asset in the pool through the normalized value.

---

## Interface

### Create pool

To crate a pool, it requires sending [`MsgCreateCosmWasmPool`](https://github.com/osmosis-labs/osmosis/blob/b94dbe643ff37d93a639f49ee9b81208dbc3ba8c/proto/osmosis/cosmwasmpool/v1beta1/model/tx.proto#L14-L20) which includes `code_id` and `instantiation_msg` for the contract, `cosmwasmpool` module will instantiate the contract under the hood and register the pool.

`instantiation_msg` example:

```rs
{
    "pool_asset_configs": [
      { "denom": "ibc/a..", "normalization_factor": "1000000" },
      { "denom": "ibc/b..", "normalization_factor": "1" }
    ],
    "alloyed_asset_subdenom": "alloyed",
    "alloyed_asset_normalization_factor": "1",
    "admin": "osmo1a..",
    "moderator": "osmo1b.."
}
```

pool_asset_configs: Vec<AssetConfig>,
alloyed_asset_subdenom: String,
alloyed_asset_normalization_factor: Uint128,
admin: Option<String>,
moderator: Option<String>,

- `pool_asset_denoms` - list of denoms that will be used as pool assets
- `alloyed_asset_subdenom` - subdenom of the alloyed asset, the resulted denom will be `factory/{contract_address}/{alloyed_asset_subdenom}`
- `admin` - admin address of the contract, it can be transferred later

### Join and Exit pool

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

### Swap

The swap can be performed through [`poolmanager`'s msgs](https://github.com/osmosis-labs/osmosis/tree/main/x/poolmanager#swaps) which will get routed to the contract's sudo entrypoint.

### Administration

Admin address can be set on instantiation of the contract. The admin can be changed by sending:

```json
{ "transfer_admin": { "candidate": "osmo1..." } }
```

Only candidate can claim the admin role by sending:

```json
{ "claim_admin": {} }
```

The following are admin only operations:

- [`Set Active Status`](#set-active-status)
- [`Set Alloyed Denom Metadata`](#set-alloyed-denom-metadata)
- [`Register, Update and Deregister Limiters`](#register-update-and-deregister-limiters)

#### Set Active Status

As for now, `cosmwasmpool` module still haven't route pool deactivation to the contract. So for now the only way to deactivate the pool is to have an admin who can deactivate the pool.

Admin is set on instantiate and can send the following msg:

```json
{ "set_active_status": true }
```

With deactivation, the pool will not be able to accept any execute or sudo request except for `set_active_status`.

#### Set Alloyed Denom Metadata

Set metadata for alloyed denom.

```json
{
  "set_alloyed_denom_metadata": {
    "base": "alloyed",
    "description": "Alloyed Asset",
    "display": "alloyed",
    "name": "Alloyed Asset",
    "symbol": "ALD",
    "denom_units": [
      {
        "denom": "alloyed",
        "exponent": 0,
        "aliases": []
      },
      {
        "denom": "ualloyed",
        "exponent": 6,
        "aliases": []
      }
    ]
  }
}
```

#### Register, Update and Deregister Limiters

`register_limiter` can be used to register a new limiter.

Example for registering change limiter:

```json
{
  "register_limiter": {
    "denom": "token1",
    "label": "1h",
    "limiter_params": {
      "change_limiter": {
        "window_config": {
          "window_size": "3600000000000",
          "division_count": "5"
        },
        "boundary_offset": "0.2"
      }
    }
  }
}
```

Example for registering static limiter:

```json
{
  "register_limiter": {
    "denom": "token1",
    "label": "static",
    "limiter_params": {
      "static_limiter": {
        "upper_limit": "0.7"
      }
    }
  }
}
```

For updating limiter params, change limiter's boundary offset can be updated:

```json
{
  "set_change_limiter_boundary_offset": {
    "denom": "token1",
    "label": "1h",
    "boundary_offset": "0.2"
  }
}
```

and static limiter's upper limit can be updated:

```json
{
  "set_change_limiter_boundary_offset": {
    "denom": "token1",
    "label": "static",
    "upper_limit": "0.8"
  }
}
```

Apart from existing ways for updating limiter params, to change any other parameters like `window_size`, ones need to deregister the limiter and register it again with the new params since those operations requires reconfiguring stored data.

`deregister_limiter` can be used to deregister the limiter.

```json
{
  "deregister_limiter": {
    "denom": "token1",
    "label": "1h"
  }
}
```

#### Assets Management

Admin can `add_new_assets`

```json
{
  "add_new_assets": {
    "assets": [
      { "denom": "ibc/a..", "normalization_factor": "1000000" },
      { "denom": "ibc/b..", "normalization_factor": "1" }
    ]
  }
}
```

`rescale_normalization_factor` which will multiply the normalization factor of each asset with the given factor.
This is needed if the soon-to-be added asset requires readjustment of the normalization factor due to `LCM` of the old asset composition differs from the new one.

```json
{
  "rescale_normalization_factor": {
    "numerator": "10000",
    "denominator": "1"
  }
}
```

and moderator can `mark_corrupted_assets` which is needed for [Risk and Mitigation](#risk-and-mitigation) strategy.

```json
{
  "mark_corrupted_assets": {
    "assets": ["ibc/a.."]
  }
}
```

## Access Control List

There are 2 special roles in the contract:

- `Admin` - Can perform pool management tasks
- `Moderator` - Can perform incidence response tasks

| Execute Message \ Authorized Role    | Admin | Moderator | Admin Candidate |
| ------------------------------------ | ----- | --------- | --------------- |
| `rescale_normalization_factor`       | ✓     |           |                 |
| `add_new_assets`                     | ✓     |           |                 |
| `mark_corrupted_assets`              |       | ✓         |                 |
| `unmark_corrupted_assets`            |       | ✓         |                 |
| `register_limiter`                   | ✓     |           |                 |
| `deregister_limiter`                 | ✓     |           |                 |
| `set_change_limiter_boundary_offset` | ✓     |           |                 |
| `set_static_limiter_upper_limit`     | ✓     |           |                 |
| `set_alloyed_denom_metadata`         | ✓     |           |                 |
| `set_active_status`                  |       | ✓         |                 |
| `transfer_admin`                     | ✓     |           |                 |
| `cancel_admin_transfer`              | ✓     |           |                 |
| `reject_admin_transfer`              |       |           | ✓               |
| `claim_admin`                        |       |           | ✓               |
| `renounce_adminship`                 | ✓     |           |                 |
| `assign_moderator`                   | ✓     |           |                 |
| `remove_moderator`                   | ✓     |           |                 |

Apart from the table above, other execute messages has no role restrictions.

## Risk and Mitigation

Each underlying asset that makes up the pool (therefore the alloyed asset's value) has it's own risk. In case one of the asset has depegged, it can lead to the loss of the pool's value.

To limit the risk, each asset [limiters](#limiters) mentioned above to limit the risk and giving time for pool moderator to react.

In case of a depegged asset, the pool moderator can mark the asset as `corrupted`, what's following is that, the said asset:

- can only decrease in both amount and weight, which means, if user wants to redeem another non-corrupted asset, user must redeem equal amount of corrupted asset (because not doing that will make corrupted asset weight increases)
- in case there is an altruistic actor that wishes to redeem all corrupted asset, they can do so in single transaction and ignore all the limiters
- Once corrupted assets reaches 0, it will be removed from the pool and resume its operation
- The limiters will require new setting afterwards since asset weight will no longer account for removed assets
