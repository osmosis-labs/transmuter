import streamlit as st
import random
import plotly.express as px
import numpy as np
import pandas as pd


from limiters import ChangeLimiter, StaticLimiter
from pool import Pool

"""
# Transmuter Limiter Simulation
"""


class Simulation:
    def __init__(self, denoms: list[str]):
        self.pool = Pool(denoms)
        self.actions = [self.pool.join_pool, self.pool.exit_pool, self.pool.swap]
        self.denoms = self.pool.denoms()
        self.snapshots: pd.DataFrame = pd.DataFrame(
            columns=["denom", "timestamp", "amount", "weight"]
        )

    def run(
        self,
        timesteps: int,
        max_action_count: int,
        amount_mean: float,
        amount_sd: float,
    ):
        latest_timestamp: int = (
            self.snapshots["timestamp"].max(skipna=True)
            if not self.snapshots.empty
            else 0
        )
        denoms = self.pool.denoms()

        for timestamp in range(
            latest_timestamp,
            latest_timestamp + timesteps,
        ):  # Adjust this to the number of iterations you want
            # Randomly choose 0-10 action count for each timestamp
            action_count = random.randint(0, int(max_action_count))

            for _ in range(action_count):
                # Choose random action and denom
                action = random.choice(
                    [self.pool.join_pool, self.pool.exit_pool, self.pool.swap]
                )
                denom = random.choice(denoms)

                # Generate random amount
                # using log normal distribution due to positive only nature of amount
                amount = np.log(random.lognormvariate(amount_mean, amount_sd))

                # ensure that amount is not negative
                if amount < 0:
                    amount = 0

                # Perform action
                if action == self.pool.swap:
                    # Choose a random denom_out that is different from denom_in
                    denom_out = random.choice([d for d in denoms if d != denom])

                    # ensure that swap is possible
                    if self.pool.assets[denom_out] < amount:
                        amount = self.pool.assets[denom_out]

                    # Perform the swap action with denom_in, denom_out and the generated amount
                    self.pool.swap(denom, denom_out, timestamp, amount)

                elif action == self.pool.join_pool:
                    count = random.randint(1, len(denoms))
                    self.pool.join_pool(
                        timestamp,
                        {denom: amount for denom in random.sample(denoms, count)},
                    )

                elif action == self.pool.exit_pool:
                    count = random.randint(1, len(denoms))
                    _denoms = random.sample(denoms, count)

                    self.pool.exit_pool(
                        timestamp,
                        {
                            denom: min(amount, self.pool.assets[denom])
                            for denom in _denoms
                        },
                    )

                else:
                    raise Exception("Action not implemented")

            # Record snapshot
            new_snapshots = pd.DataFrame(
                [
                    {
                        "denom": denom,
                        "timestamp": timestamp,
                        "amount": self.pool.assets[denom],
                        "weight": self.pool.weight(denom),
                    }
                    for denom in denoms
                ]
            )
            self.snapshots = (
                pd.concat(
                    [
                        self.snapshots,
                        new_snapshots,
                    ],
                    ignore_index=True,
                )
                if not self.snapshots.empty
                else new_snapshots
            )


def init_state(reset=False):
    if "simulation" not in st.session_state or reset:
        st.session_state.simulation = Simulation(["denom1", "denom2"])
        # TODO: check why order of limiters matter
        st.session_state.simulation.pool.set_limiters(
            "denom1", [StaticLimiter(0.6), ChangeLimiter(0.001, 10)]
        )
        st.session_state.simulation.pool.set_limiters(
            "denom2", [StaticLimiter(0.6), ChangeLimiter(0.001, 10)]
        )


init_state()


with st.sidebar:
    timesteps = st.number_input(
        "time steps", min_value=1, max_value=10000, value=1000, step=1, key="timesteps"
    )
    max_action_count = st.number_input(
        "max action count",
        min_value=1,
        max_value=1000,
        value=10,
        step=1,
        key="max_action_count",
    )
    amount_mean = st.number_input(
        "amount mean", min_value=1, max_value=1000000, value=10, key="amount_mean"
    )
    amount_sd = st.number_input(
        "amount sd", min_value=1, max_value=1000000, value=100, key="amount_sd"
    )

    if st.button("Simulate more"):
        st.session_state.simulation.run(
            timesteps, max_action_count, amount_mean, amount_sd
        )

    if st.button("Reset"):
        init_state(reset=True)

snapshots = st.session_state.simulation.snapshots
if not snapshots.empty:
    with st.expander("Show raw simulation snapshots"):
        st.write(snapshots)

    # Plot amount over time
    fig = px.line(
        snapshots,
        x="timestamp",
        y="amount",
        color="denom",
        title="Amount over time",
    )

    st.plotly_chart(fig)

    # Plot weight over time
    fig = px.line(
        snapshots,
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
