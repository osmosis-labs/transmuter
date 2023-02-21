# Transmuter

A CosmWasm contract to enable 1-direction 1:1 conversion of one asset to another.

## Interacting with the contract

### Setup

To set up the contract, contract instantiation is needed with the following information:

```rs
pub struct InstantiateMsg {
    /// denoms of the tokens that can be reserved in the pool and can be transmuted.
    /// must contain at least 2 denoms.
    pool_asset_denoms: Vec<String>,
}
```

### Join/Exit Pool

A user can join the pool by sending a `JoinPool` message to the contract. The message also has no field and the amount of tokens to be added to the reserve is taken from the `MsgExecuteContract` funds.
Token denoms in the funds must be one of the denoms in the `pool_asset_denoms` field of the `InstantiateMsg`.

```rs
pub struct JoinPool {}
```

A user can exit the pool by sending a `ExitPool` message to the contract. As long as the sender has enough shares, the contract will send `tokens_out` amount of tokens to the sender.
The amount of shares will be deducted from the sender's shares equals to sum of the amount of tokens_out.

```rs
pub struct ExitPool {
  pub tokens_out: Vec<Coin>
}
```

### Transmutation

A transmutation is done by sending a `Transmute` message to the contract. The amount of tokens to be transmuted is taken from the `MsgExecuteContract` funds the same way `JoinPool` message is except that it requires `token_out_denom` to be one of the denoms in the `pool_asset_denoms` field of the `InstantiateMsg`.

```rs
pub struct Transmute {
  pub token_out_denom: String,
}
```
