import streamlit as st
import random
import plotly.express as px
import numpy as np


from limiters import ChangeLimiter, StaticLimiter
from pool import Pool

"""
# Transmuter Limiter Simulation
"""


def init_state(reset=False):
    if "pool" not in st.session_state or reset:
        st.session_state.pool = Pool(["denom1", "denom2"])
        # TODO: check why order of limiters matter
        st.session_state.pool.set_limiters(
            "denom1", [StaticLimiter(0.6), ChangeLimiter(0.001, 10)]
        )
        st.session_state.pool.set_limiters(
            "denom2", [StaticLimiter(0.6), ChangeLimiter(0.001, 10)]
        )

    denoms = st.session_state.pool.denoms()

    # Initialize dictionaries to store weights and timestamps for each denom
    if "total_time_steps" not in st.session_state or reset:
        st.session_state.total_time_steps = 0

    if "weights" not in st.session_state or reset:
        st.session_state.weights = {denom: [] for denom in denoms}
    if "amounts" not in st.session_state or reset:
        st.session_state.amounts = {denom: [] for denom in denoms}
    if "timestamps" not in st.session_state or reset:
        st.session_state.timestamps = {denom: [] for denom in denoms}


init_state()

pool = st.session_state.pool
actions = [pool.join_pool, pool.exit_pool, pool.swap]
denoms = pool.denoms()

with st.sidebar:
    timesteps = st.number_input("time steps", min_value=1, max_value=10000, value=1000)
    max_action_count = st.number_input(
        "max action count", min_value=1, max_value=1000, value=10
    )
    amount_mean = st.number_input(
        "amount mean", min_value=1, max_value=1000000, value=10
    )
    amount_sd = st.number_input("amount sd", min_value=1, max_value=1000000, value=100)

    if st.button("Simulate more"):
        # Run simulation
        for timestamp in range(
            st.session_state.total_time_steps,
            st.session_state.total_time_steps + timesteps,
        ):  # Adjust this to the number of iterations you want
            # Randomly choose 0-10 action count for each timestamp
            action_count = random.randint(0, max_action_count)

            for _ in range(action_count):
                # Choose random action and denom
                action = random.choice(actions)
                denom = random.choice(denoms)

                # Generate random amount
                # using log normal distribution due to positive only nature of amount
                amount = np.log(random.lognormvariate(amount_mean, amount_sd))

                if amount < 0:
                    amount = 0

                # # Perform action
                if action == pool.swap:
                    # Choose a random denom_out that is different from denom_in
                    denom_out = random.choice([d for d in denoms if d != denom])

                    # ensure that swap is possible
                    if pool.assets[denom_out] < amount:
                        amount = pool.assets[denom_out]

                    # Perform the swap action with denom_in, denom_out and the generated amount
                    pool.swap(denom, denom_out, timestamp, amount)

                elif action == pool.join_pool:
                    count = random.randint(1, len(denoms))
                    pool.join_pool(
                        timestamp,
                        {denom: amount for denom in random.sample(denoms, count)},
                    )

                elif action == pool.exit_pool:
                    count = random.randint(1, len(denoms))
                    _denoms = random.sample(denoms, count)

                    pool.exit_pool(
                        timestamp,
                        {denom: min(amount, pool.assets[denom]) for denom in _denoms},
                    )

                else:
                    raise Exception("Action not implemented")

            # Record weight and timestamp for each denom
            for denom in denoms:
                st.session_state.weights[denom].append(pool.weight(denom))
                st.session_state.amounts[denom].append(pool.assets[denom])
                st.session_state.timestamps[denom].append(timestamp)

        st.session_state.total_time_steps += timesteps

    if st.button("Reset"):
        init_state(reset=True)

    # Construct dataframe from timestamps, amounts, and weights
import pandas as pd

data = []
for denom in denoms:
    for timestamp, amount, weight in zip(
        st.session_state.timestamps[denom],
        st.session_state.amounts[denom],
        st.session_state.weights[denom],
    ):
        data.append(
            {"denom": denom, "timestamp": timestamp, "amount": amount, "weight": weight}
        )


df = pd.DataFrame(data)

with st.expander("Show raw simulation data"):
    st.write(df)


if not df.empty:
    # Plot amount over time
    fig = px.line(
        df,
        x="timestamp",
        y="amount",
        color="denom",
        title="Amount over time",
    )

    st.plotly_chart(fig)

    # Plot weight over time
    fig = px.line(
        df,
        x="timestamp",
        y="weight",
        color="denom",
        title="Weight over time",
    )

    st.plotly_chart(fig)


# TODO:
# - make limiter configurable
# - inject simulation with config
# - add new denom and see how simulation perform
