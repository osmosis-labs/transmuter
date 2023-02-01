# Transmuter

A CosmWasm contract to enable 1-direction 1:1 conversion of one asset to another.

## Interacting with the contract

### Setup

To set up the contract, contract instantiation is needed with the following information:

```rs
pub struct InstantiateMsg {
    /// the denom of the coin to be transmuted.
    pub in_denom: String,
    /// the denom of the coin that is transmuted to, needs to be supplied to the contract.
    pub out_denom: String,
    /// the admin of the contract, can change the admin and withdraw funds.
    pub admin: String,
}
```

### Supplying out_denom reserve

The contract needs a reserve of out denom coins to be able to transmute. Coins can be added to the reserve by sending a `Supply` message to the contract.
`Supply` messages has no field and the amount of coins to be added to the reserve is taken from the `MsgExecuteContract` funds.

```rs
Supply {}
```

### Transmutation

A transmutation is done by sending a `Transmute` message to the contract. The message also has no field and the amount of coins to be transmuted is taken from the `MsgExecuteContract` funds the same way `Supply` message is.
The contract will expect `in_denom` coins to be sent and will send `out_denom` coins to the message sender with the same amount as the `in_denom` coins sent.

```rs
Transmute {}
```

### Admin

Admin can be changed by sending a `UpdateAdmin` message to the contract with the new admin address as the field.

```rs
UpdateAdmin {
    new_admin: String,
}
```

Only admin can withdraw both `in_denom` and `out_denom` coins from the contract by sending a `Withdraw` message to the contract.

```rs
Withdraw {
    coins: Vec<Coin>,
}
```
