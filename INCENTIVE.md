# Alloyed Asset Rebalancing Incentive Mechanism


Currently, underlying assets for alloyed asset has limiter to limit its risk from bridge/chain hack but there is no incentives for any actor to act on if any of the asset hitting the limit or reaching 0.

## Overview

The goal is to ensure that if a swap moves the asset composition closer to a set limit on the bridge/chain, fee will be charged. However, if the swap moves the asset composition towards balance, it will receive an incentive.

### Parameters

| symbol           | definition                                                                             |
| ---------------- | -------------------------------------------------------------------------------------- |
| $\vec{\delta}$   | upper limit, $\delta \in (0,1]$                                                        |
| $\vec{\phi_u}$   | ideal balance upper bound, $\phi_u \in (\phi_l,\delta)$                                |
| $\vec{\phi_l}$   | ideal balance lower bound, $\phi_l \in (0, \phi_u)$                                    |
| $\vec{\kappa_u}$ | critical balance upper bound, $\kappa_u \in (\phi_u, \delta)$                          |
| $\vec{\kappa_l}$ | critical balance lower bound, $\kappa_l \in (0,\phi_l)$                                |
| $\vec{r_{s}}$    | base fee/incentive rate within $[\kappa_l, \phi_l] \cup [\phi_u, \kappa_u]$ (strained) |
| $\vec{r_c}$      | base fee/incentive rate within $[0, \kappa_l) \cup (\kappa_u, \delta]$ (critical)      |

### Variables

| symbol      | definition                                                                                                                                                                                                                              |
| ----------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| $\vec{b}$   | balance before swap, $b \in [0,1]$                                                                                                                                                                                                      |
| $\vec{b'}$  | balance after swap, $b' \in [0,1]$                                                                                                                                                                                                      |
| $B_{total}$ | Total balance in the liquidity pool normalized by [standard normalization factor](https://github.com/osmosis-labs/transmuter/blob/b7f0f76e443053ca755873c288437ce7492ade28/contracts/transmuter/src/transmuter_pool/weight.rs#L14-L22).

All the vectors are $n$ dimensional where $n$ is number of source bridge/chain.


## Calculation

<iframe src="https://www.desmos.com/calculator/skl3b3mppa?embed" width="500" height="500" style="border: 1px solid #ccc" frameborder=0></iframe>

Each asset will have their own $\delta, \phi_l, \phi_u, \kappa_l, \kappa_u, r_s, r_c$ parameter, when balance changes, for each assets we determine the value $v$ that is a normalized value (by [standard normalization factor](https://github.com/osmosis-labs/transmuter/blob/b7f0f76e443053ca755873c288437ce7492ade28/contracts/transmuter/src/transmuter_pool/weight.rs#L14-L22)) of the pool asset.



> check out [this interactive visualization](https://alloyed-asset-rebalancing.streamlit.app/) to see how it behaves visually 

```python
def compute_v(
    b: float,
    b_prime: float,
    B_total: float,
    phi_l: float,
    phi_u: float,
    kappa_l: float,
    kappa_u: float,
    delta: float,
    r_s: float,
    r_c: float,
) -> float:
    """Compute fee or incentive adjustment for a single asset's balance movement.
    
    This function calculates the incentive/rebate (if positive) or fee (if negative)
    for a swap that moves an asset's balance from b to b_prime. The goal is to
    encourage movements toward the ideal balance range [phi_l, phi_u] and 
    discourage movements away from it.
    """
    
    # Define the five zones with their respective fee/incentive rates
    # The zones create a graduated incentive structure:
    # - Critical zones (highest rates): Danger zones where we strongly want to avoid
    # - Strained zones (moderate rates): Warning zones approaching critical levels  
    # - Ideal zone (zero rate): Target range where we want balances to be
    # This creates a "gravity well" effect pulling balances toward ideal range
    zones = [
        (0.0,     kappa_l, r_c),   # critical low: [0, kappa_l) - highest incentive to move out
        (kappa_l, phi_l,   r_s),   # strained low: [kappa_l, phi_l) - moderate incentive to move up
        (phi_l,   phi_u,   0.0),   # ideal zone: [phi_l, phi_u] - neutral, no fees or incentives
        (phi_u,   kappa_u, r_s),   # strained high: (phi_u, kappa_u] - moderate incentive to move down
        (kappa_u, delta,   r_c),   # critical high: (kappa_u, delta] - highest incentive to move out
    ]

    v = 0.0  # Initialize total fee/incentive accumulator

    # Process each zone separately because a single swap can cross multiple zones
    # Example: A swap from 15% to 35% might cross critical low → strained low → ideal
    # Each zone contributes its portion with its specific rate
    for z_start, z_end, rate in zones:
        # Get the portion of the swap path that overlaps with this zone
        # This ensures we only account for movement within each specific zone
        seg_start, seg_end = get_segment_overlap(b, b_prime, z_start, z_end)

        if seg_end <= seg_start:
            continue  # No overlap with this zone, skip to next
        
        # Determine if movement in this zone is toward ideal (+1), away from ideal (-1), or neutral (0)
        # The direction logic considers:
        # - If we're below ideal and moving up → good (+1)
        # - If we're above ideal and moving down → good (+1)
        # - If we're below ideal and moving further down → bad (-1)
        # - If we're above ideal and moving further up → bad (-1)
        # - Movement within ideal zone → neutral (0)
        direction = get_direction(b, b_prime, z_start, z_end, phi_l, phi_u)
        
        # Calculate the contribution from this zone:
        # - segment_length: How much of the balance change happened in this zone
        # - rate: The fee/incentive rate for this zone (higher for critical zones)
        # - direction: Whether this movement is beneficial (+1) or harmful (-1)
        # The multiplication gives us a signed value where:
        # - Positive = incentive/rebate to be given (movement toward ideal)
        # - Negative = fee to be charged (movement away from ideal)
        segment_length = seg_end - seg_start
        v += direction * rate * segment_length

    # Scale by total pool balance to convert from percentage-based calculation 
    # to actual token amounts. This makes fees/incentives proportional to pool size:
    # larger pools → larger absolute fees/incentives for the same percentage movement
    return v * B_total
```

where


```python
def get_segment_overlap(b: float, b_prime: float, z_start: float, z_end: float) -> tuple[float, float]:
    """Returns the overlapping segment [seg_start, seg_end] of the swap path within the given zone.
    
    When a balance moves from b to b_prime, it traces a path. This function finds
    what portion of that path falls within a specific zone [z_start, z_end].
    This is crucial because different zones have different fee/incentive rates.
    """
    # The swap path is from min(b, b_prime) to max(b, b_prime)
    # regardless of direction (increasing or decreasing balance)
    swap_start = min(b, b_prime)
    swap_end = max(b, b_prime)
    
    # Find the intersection of swap path with the zone
    # seg_start: The later of swap start or zone start
    # seg_end: The earlier of swap end or zone end
    seg_start = max(swap_start, z_start)
    seg_end = min(swap_end, z_end)
    
    # If seg_end <= seg_start, there's no overlap (will be handled by caller)
    return seg_start, seg_end


def get_direction(b: float, b_prime: float, z_start: float, z_end: float, phi_l: float, phi_u: float) -> int:
    """Returns +1 if movement in zone is toward ideal, -1 if away, 0 if neutral.
    
    This function determines whether a balance movement within a specific zone
    should be encouraged (toward ideal balance) or discouraged (away from ideal).
    The ideal range is [phi_l, phi_u], and we want to incentivize movements
    that bring the balance closer to or keep it within this range.
    """
    if b == b_prime:
        return 0  # No movement means no fee or incentive
    
    # Given the zones in `compute_v`, a zone is always either
    # entirely below, entirely above, or exactly matching the ideal range.
    # We classify the zone's position relative to this ideal range.
    is_below_ideal = z_end <= phi_l      # Zone is entirely below ideal
    is_above_ideal = z_start >= phi_u    # Zone is entirely above ideal
    is_ideal_zone = z_start == phi_l and z_end == phi_u  # Zone exactly matches ideal range

    # If we're in ideal zone, any movement is neutral
    if is_ideal_zone:
        return 0
    
    # For rightward movement (increasing balance)
    if b_prime > b:  
        # If we're below ideal, moving right (up) is good - approaching ideal
        if is_below_ideal:
            return +1
        # If we're above ideal, moving right (up) is bad - moving away from ideal
        if is_above_ideal:
            return -1

        # Otherwise we're in a zone that partially overlaps with ideal - neutral
        return 0
    
    # For leftward movement (decreasing balance)
    if b_prime < b:
        # If we're above ideal, moving left (down) is good - approaching ideal
        if is_above_ideal:
            return +1
        # If we're below ideal, moving left (down) is bad - moving away from ideal
        if is_below_ideal:
            return -1
        # Otherwise we're in a zone that partially overlaps with ideal - neutral
        return 0

```

This will give, for each pool asset, the normalized fee/incentive amount. Then we calculate

$\sum{v}$.

If $\sum{v} > 0$ then it's an incentive, $\sum{v} < 0$ then it collects fee, does nothing otherwise.

## Swapping alloyed assets (join/exit pool)

When a swap involves adding or removing liquidity (e.g., swapping alloyed assets in or out), `B_total` changes. This requires a nuanced approach to determine whether to use `B_total` before the swap (`B_total_before`) or after the swap (`B_total_after`) for the fee/incentive calculation.

A hybrid approach is used to ensure both fairness and security:

1.  **If the swap is beneficial (results in an incentive):** The calculation is scaled by `B_total_before`.
    -   **Justification**: Incentives are paid from a pool of previously collected fees. It is logical to scale the reward based on the state of the pool *before* the user's helpful contribution. This provides a fair reward relative to the pool's history and prevents a single large, helpful deposit from draining a disproportionate amount of the incentive fund.

2.  **If the swap is harmful (results in a fee):** The calculation is scaled by `max(B_total_before, B_total_after)`.
    -   **Justification**: This provides maximum security for the pool.
        -   For **harmful joins** (liquidity addition), the fee is based on `B_total_after`. This ensures the penalty is proportional to the new, larger pool size that the user has unbalanced.
        -   For **harmful exits** (liquidity withdrawal), the fee is based on `B_total_before`. This ensures the penalty is proportional to the state of the pool *before* it was damaged by the withdrawal.

This model robustly protects the pool against destabilizing actions while ensuring that incentive payouts are scaled conservatively and fairly.

This is a consideration per asset, meaning, each `compute_v` can have different `B_total`.

## Swap Semantics

Swaps need to conform either `SwapExactAmountIn` or `SwapExactAmountOut` and we need to consider how that effects our fee/incentive calculation, token to be collected as fee into incentive pool and token to be distributed as incentive.

### Swap is harmful and we collect fees
- `SwapExactAmountIn` needs to keep the amount in constant, because if we deduct fee from amount in, the $v$ calculation will have circular dependency. So we deduct the amount out and put that into incentive pool.

- `SwapExactAmountOut` needs to keep the amount out constant for the same reason as above, so we need to factor in the fee amount into amount in, meaning, if $a_{in}$ is used to compute $\sum{v}$, effective amount in that user eventually have to pay is $a_{in} + fee$, and collect that fee portion into incentive pool.


### Swap is beneficial and we distribute incentive
The simpliest solution here is to make the pool in debt to the swapping account and allow claiming later.

The reason here is that, we can't control or predict the token that will end up in the incentive pool, only its value that we have some control over. Because a swap can, for example:

- `SwapExactAmountIn` from A -> B
- make A's position worse
- make B's position better
- total is worse, so collect fee
- fee collected in B (token out)

as oppose to the same balance shift but `SwapExactAmountOut` instead, the collected fee will be in A.

It's the same effect to the liquidity pool but it's collecting different token to the incentive pool (eventhough the total value is the same).

If we always have enough amount to pay incentive as token out (on `SwapExactAmountIn`) or as subsidizing token in (on `SwapExactAmountOut`), that would be ideal, because it's obvious to user (and any kind of swap route provider) when a swap is incentivized, but we can't gurantee that.

So making the pool in debt, and the interacting account can later claim their incentive in any token available in the incentive pool is simpliest and most effective given we are building bot to do the rebalancing.

Total debt must never exceed total incentive pool, if such case happen when calculating an incentive, deduct the excess incentive from that swap.

## When the incentive pool doesn't have enough to incentivize the rebalance

If 

$$
\text{total incentive pool} - \text{outstanding debt}
$$

is less than 

$$
\sum{v(b,argmin_{\phi}{(|b-\phi|)})}; \phi \in \{\phi_l, \phi_u\}
$$

> This reads: total incentive that needs to pay out when all the out-of-ideal-zone balance moved into ideal zone.

Then raise all fee rate to $r_c$ until it recovers, so that we can make sure we have large enough incentive pool to incentivize all rebalance.

## Asset Groups

When dealing with asset groups, we need to consider how balance changes affect both individual assets and their groups. Let's consider a scenario with:
- Asset group $G$ containing denoms $d_1$ and $d_2$
- A separate denom $d'$ not in group $G$

The fee/incentive calculation follows these rules:

1. **Intra-group swaps** (e.g., $d_1 \leftrightarrow d_2$):
   - The group-level calculation $v_G(b_G, b'_G) = 0$ always
   - This is because swapping between assets in the same group doesn't affect the group's overall balance
   - Individual asset calculations $v_{d_1}$ and $v_{d_2}$ are still performed as normal

2. **Inter-group swaps** (e.g., $d_1 \leftrightarrow d'$):
   - Calculate group-level effect: $v_G(b_G, b'_G)$
   - Calculate individual asset effects: $v_{d_1}(b_{d_1}, b'_{d_1})$ and $v_{d'}(b_{d'}, b'_{d'})$
   - The final fee/incentive is the sum of these values
   - Note: This means the same balance change can contribute to both group-level and individual asset-level incentives

This approach allows us to:
- Maintain separate incentive mechanisms for both individual assets and their groups
- Ensure that balance changes are properly incentivized at both levels
- Handle cases where a swap might be beneficial for one level but harmful for another

## Handling Corrupted Asset

When any assets marked as corrupted, override the following params for those assets:

$$
\kappa_l, \phi_l, \phi_u, \kappa_u = 0
$$

$$
\delta = b
$$

If it's a corrupted asset group, all underlying assets in the the asset group gets overwritten the same way as above.

Corrupted in the incentive pool also require claiming until it reaches 0 

This will automatically incentivize all action that decrease corrupted assets.