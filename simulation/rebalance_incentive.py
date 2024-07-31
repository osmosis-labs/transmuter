import streamlit as st
import streamlit as st
import plotly.graph_objects as go
import numpy as np

r'''
# Alloyed Asset Rebalancing Incentive Mechanism

Currently, underlying assets for alloyed asset has limiter to limit its risk from bridge/chain hack but there is no incentives for any actor to act on if any of the asset hitting the limit or reaching 0.

## Overview

The goal is to ensure that if a swap moves the asset composition closer to a set limit on the bridge/chain, fee will be charged. However, if the swap moves the asset composition towards balance, it will receive an incentive.

### Parameters

| symbol | definition | 
| -------- | -------- | 
| $\vec{\delta_{u}}$ | upper cap ratio |
| $\vec{\delta_{l}}$ | lower cap ratio |
| $k$ | power factor for scaling fee & incentive, $k \in [0, \infty)$ |
| $\lambda$ | fee scaler, $\lambda \in (0,1]$ |

Constraint: $\forall{i} \in \{1 \dots n\}; \delta_{u_i} > \delta_{l_i}$

Both of the caps are $n$ dimensional vectors, where $n$ is the total number of source bridge/chain we want to put limits on.

### Variables

| symbol | definition | 
| -------- | -------- |
| $\vec{b}$ | current balance |
| $\vec{\Delta b}$ | changes in balance for current swap |
| $B$ | total balance |

$\vec{b}$ and $\vec{\Delta b}$ are also $n$ dimensional vectors, while $B$ is scalar. All the balances here are assumed to be normalized by normalization factor (eg. $b_1 = 99$ and $b_2 = 99$ is regarded as having the same monetary value).


With that, calculate updated balance:

$$
\vec{b'} = \vec{b} + \vec{\Delta b}
$$
$$
B' = \vec{1} \cdot \vec{b'}
$$

And because  $\vec{\delta_u}, \vec{\delta_l}$ are ratio, we need to transform balance into ratio as well:
$$
\vec{b_r} = \frac{\vec{b}}{B}
$$
$$
\vec{b_r'} = \frac{\vec{b'}}{B'}
$$
$$
\vec{\Delta b_r} = \vec{b_r'} - \vec{b_r}
$$

Let midpoint $\vec{m}$ be the point where it sits in the middle between 2 boundaries

$$
\vec{m} = \frac{\vec{\delta_{u}}+\vec{\delta_{l}}}{2}
$$

Since $\vec{b_r}$ sits on a hyperplane $\vec{1} \cdot \vec{b_r} = 1$ which has $\vec{1}$ as normal vector, we want to project $\vec{m}$ onto this plane, so that it serves as an origin for fee/incentive scaling.

$$
\vec{m_p} = \vec{m} - (\frac{\vec{1} \cdot \vec{m} - 1}{n})\vec{1}
$$
'''

# Define the function to project the point onto the line x + y = 1
def project_point(m):
    n = len(m)
    m_dot_1 = np.dot(np.ones(n), m)
    m_p = m - ((m_dot_1 - 1) / n) * np.ones(n)
    return m_p

# Create sliders for user to manually change the values of m
m_x = st.slider('m_x', min_value=0.0, max_value=1.0, step=0.01, value=0.5)
m_y = st.slider('m_y', min_value=0.0, max_value=1.0, step=0.01, value=0.5)

# Create the vector m and project it
m = np.array([m_x, m_y])
m_p = project_point(m)

# Create the figure
fig = go.Figure()

# Add the line x + y = 1
x = np.linspace(0, 1, 100)
y = 1 - x
fig.add_trace(go.Scatter(x=x, y=y, mode='lines', name='x + y = 1'))

# Add the original point m
fig.add_trace(go.Scatter(x=[m[0]], y=[m[1]], mode='markers', name='m', marker=dict(color='red', size=10)))

# Add the projected point m_p
fig.add_trace(go.Scatter(x=[m_p[0]], y=[m_p[1]], mode='markers', name='m_p', marker=dict(color='blue', size=10)))

# Add the projection vector from m to m_p
fig.add_trace(go.Scatter(x=[m[0], m_p[0]], y=[m[1], m_p[1]], mode='lines', name='projection', line=dict(color='white', dash='dot', width=1)))


# Set the layout
fig.update_layout(title='Midpoint Projection',
                  xaxis_title='x',
                  yaxis_title='y',
                  xaxis=dict(range=[0, 1]),
                  yaxis=dict(range=[0, 1], scaleanchor="x", scaleratio=1),
                  showlegend=True)


# add fig to st.plotly_chart
st.plotly_chart(fig)


r'''
If $\vec{b_r}$ is moving towards $\vec{m_p}$ then it's a balancing act, that swap will be incentivized, otherwise it will incur fee. Let $d$ be a distance from projected midpoint before swap and $d'$ be a distance from projected midpoint after swap.



$$
d = \|\vec{b_r} - \vec{m_p}\|
$$
$$
d' = \|\vec{b_r'} - \vec{m_p}\|
$$

$d' < d$ means it's moving closer to the midpoint, so the swap is incentivized.
$d' = d$ means not moving, so it's neither incentivized nor incurring any fee.
$d' > d$ means it's moving away from the midpoint towards one of the caps, incur fee.

There is no exception for the case where $\vec{b_r}$ and $\vec{b_r'}$ are on the different side of the midpoint, since it is still considered moving closer to the midpoint.

Fee & incentive scales along how far it moves between midpoint and caps. We want a normalized distance from midpoint as basis for the final calculation. 

To normalize, we need the maximum distance from midpoint. The intuition here is that $\vec{\delta_{u}} - \vec{\delta_{l}}$ is the diagonal vector of a hyperrectangle bounded by $\vec{\delta_{u}}$, $\vec{\delta_{l}}$ so max distance from midpoint is half of that diagonal length, but again, the real bound sits on the hyperplane  $\vec{1} \cdot \vec{b_r} = 1$ so we project $\vec{\delta_u}$, $\vec{\delta_l}$ as we project midpoint:

$$
d_{max} = \|\frac{\vec{\delta_{u_p}} - \vec{\delta_{l_p}}}{2}\|
$$

> Note: it is probably make more sense to project the boundary before calculating midpoint in the implementation

Then normalize the distances:
$$
\hat{d} = \frac{d}{d_{\max}}
$$
$$
\hat{d'} = \frac{d'}{d_{\max}}
$$


When taking the fee, take a cut from either token in or token out based on which reduce $\hat{d}'$ the most. The fee amount determined by:
$$
Fee = \lambda\Delta b(\hat{d}'^k-\hat{d}^k)
$$

For practical purpose, the fee taken is scaled by parameter $\lambda$. To put into perspective, if $\lambda$ is 1, $\hat{d'} = 1$ (max) and $\hat{d} = 0$ (min) then this whole swap is taken as fee, no token out.

![image](https://hackmd.io/_uploads/H1yrzHvt0.png)


Using higher $𝑘$ values will result in larger penalties for moves away from the midpoint.
Lower $k$ values provide a more gradual and linear impact, leading to a more moderate fee structure.

Fees are accumulated in an incentive pool $p$.

For distributing incentives from the said pool:

$$
\text{Incentive}= p \cdot (1-\hat{d}'^k)
$$

This incentive distribution will have a property that, if the resulted destination is exactly midpoint ($\hat{d}' =0$), distribute all the incentive. Since any swap right after will strictly not giving any incentive and will only collect more fee to the incentive pool.

This incentive structure encourage rational actors to quickly rebalance the pool as soon as it deviates from the midpoint, as it will require smaller amount to rebalance to get all the rewards from the incentive pool, delaying it will make room for other actor to get all the rewards.

![image](https://hackmd.io/_uploads/rkgSQSPFA.png)

Using higher $k$ values will result in larger rewards faster for moves closer to the midpoint.

The distribution starts from the asset with largest amount in the incentive pool, if not enough, goes for the 2nd largest and so on.

'''




