import streamlit as st
import random
import plotly.express as px
import numpy as np
import pandas as pd


from limiters import ChangeLimiter, StaticLimiter
from pool import Pool

st.set_page_config(
    page_title="Transmuter Simulation",
    page_icon="🔄",
    layout="centered",
    initial_sidebar_state="expanded",
)
st.title("🔄 Transmuter Simulation")


class Simulation:
    def __init__(self, denoms: list[str]):
        self.pool = Pool(denoms)
        self.actions = [self.pool.join_pool, self.pool.exit_pool, self.pool.swap]
        self.denoms = self.pool.denoms()
        self.snapshots: pd.DataFrame = pd.DataFrame(
            columns=["denom", "timestamp", "amount", "weight"]
        )

    def latest_timestamp(self):
        return (
            self.snapshots["timestamp"].max(skipna=True)
            if not self.snapshots.empty
            else 0
        )

    def join_pool(self, amount: dict[str, float]) -> bool:
        timestamp = self.latest_timestamp() + 1
        ok = self.pool.join_pool(timestamp, amount)
        self.record_snapshot(timestamp)
        return ok

    def exit_pool(self, amount: dict[str, float]) -> bool:
        timestamp = self.latest_timestamp() + 1
        ok = self.pool.exit_pool(timestamp, amount)
        self.record_snapshot(timestamp)
        return ok

    def swap(self, denom_in: str, denom_out: str, amount: float) -> bool:
        timestamp = self.latest_timestamp() + 1
        ok = self.pool.swap(denom_in, denom_out, timestamp, amount)
        self.record_snapshot(timestamp)
        return ok

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
        starting_timestamp = latest_timestamp + 1
        denoms = self.pool.denoms()

        for timestamp in range(
            starting_timestamp,
            starting_timestamp + timesteps,
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

            self.record_snapshot(timestamp)

    def record_snapshot(self, timestamp):
        denoms = self.pool.denoms()
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

        st.session_state.simulation.pool.set_limiters(
            "denom1", [ChangeLimiter(0.001, 10), StaticLimiter(0.6)]
        )
        st.session_state.simulation.pool.set_limiters(
            "denom2", [ChangeLimiter(0.001, 10), StaticLimiter(0.6)]
        )


init_state()


with st.sidebar:
    st.header("🕹️ Control Panel")

    simulation_tab, perform_action_tab, pool_config_tab = st.tabs(
        ["Simulation", "Perform Action", "Pool Config"]
    )

    with pool_config_tab:
        st.markdown("## Pool Config")
        st.markdown("### Denoms")

        st.markdown(
            "All denoms avialable in the pool, add new denoms via the button below."
        )

        if st.button("Add denom"):
            denom_count = len(st.session_state.simulation.pool.denoms())
            denom = f"denom{denom_count + 1}"
            st.session_state.simulation.pool.add_new_denom(denom)
            st.toast(f"Added `{denom}`", icon="🤌")

        st.write(
            ", ".join(
                map(
                    lambda denom: f"`{denom}`",
                    st.session_state.simulation.pool.denoms(),
                )
            )
        )

        st.markdown(
            """
        ### Limiters

        Limiters listed below are editable. Feel free to change the values or add new ones.
        Then you can try performing actions / run simulations to see if the limiters are working as expected.
        """
        )

        limiters_df = pd.DataFrame(
            columns=[
                "denom",
                "limiter_type",
                "static_upper_limit",
                "change_offset",
                "change_window_length",
            ],
            data=[
                [
                    denom,
                    limiter.__class__.__name__,
                    limiter.upper_limit
                    if limiter.__class__.__name__ == StaticLimiter.__name__
                    else None,
                    limiter.offset
                    if limiter.__class__.__name__ == ChangeLimiter.__name__
                    else None,
                    limiter.window_length
                    if limiter.__class__.__name__ == ChangeLimiter.__name__
                    else None,
                ]
                for denom, limiters in st.session_state.simulation.pool.limiters.items()
                for limiter in limiters
            ],
        )

        updated_limiters_df = st.data_editor(
            limiters_df, use_container_width=True, num_rows="dynamic"
        )

        # Iterate over all denominations in the pool
        for denom in st.session_state.simulation.pool.denoms():
            # Filter the dataframe to only include rows for the current denomination
            denom_limiters_df = updated_limiters_df[
                updated_limiters_df["denom"] == denom
            ]
            limiters = []
            # Iterate over each row in the filtered dataframe
            for _, row in denom_limiters_df.iterrows():
                # If the limiter type is StaticLimiter, create a new StaticLimiter with the specified upper limit
                if row["limiter_type"] == StaticLimiter.__name__:
                    limiters.append(StaticLimiter(row["static_upper_limit"]))
                # If the limiter type is ChangeLimiter, create a new ChangeLimiter with the specified offset and window length
                elif row["limiter_type"] == ChangeLimiter.__name__:
                    limiters.append(
                        ChangeLimiter(row["change_offset"], row["change_window_length"])
                    )
            # Set the limiters for the current denomination in the pool
            st.session_state.simulation.pool.set_limiters(denom, limiters)

    with simulation_tab:
        st.markdown(
            """
        ## Simulation

        Run the simulation with the following parameters.
        You can hit `Simulate` multiple times to generate more data points.

        Hit `Reset` to reset the simulation.
        """
        )

        timesteps = st.number_input(
            "time steps",
            help="How many times the simulation should run (resulted in newly simulated data points)",
            min_value=1,
            max_value=10000,
            value=1000,
            step=1,
            key="timesteps",
        )
        max_action_count = st.number_input(
            "max action count",
            help="Max actions per time step, each action is either join_pool, exit_pool or swap. Actual action count will be randomly generated. This is simulating the possibility of multiple actions per block.",
            min_value=1,
            max_value=1000,
            value=10,
            step=1,
            key="max_action_count",
        )
        amount_mean = st.number_input(
            "amount mean",
            help="Mean of the amount to be generated for each action. The distribution is log normal (= no negative amount).",
            min_value=1,
            max_value=1000000,
            value=10,
            key="amount_mean",
        )
        amount_sd = st.number_input(
            "amount sd",
            help="Standard deviation of the amount to be generated for each action. The distribution is log normal (= no negative amount).",
            min_value=1,
            max_value=1000000,
            value=100,
            key="amount_sd",
        )

        if st.button("Simulate"):
            st.session_state.simulation.run(
                timesteps, max_action_count, amount_mean, amount_sd
            )

        if st.button("Reset"):
            init_state(reset=True)

    with perform_action_tab:
        st.markdown("## Perform actions")

        st.markdown(
            "Perform specific actions on the pool. This is useful for forcing the pool into a desired state, or simply testing the limiters."
        )

        # create selectbox for each avaialble action
        action = st.selectbox("action", ["join_pool", "exit_pool", "swap"])
        if action == "join_pool":
            # create editable dataframe for amount
            amount_df = pd.DataFrame(
                columns=["denom", "amount"],
                data=[
                    [denom, 0.0] for denom in st.session_state.simulation.pool.denoms()
                ],
            )

            updated_amount_df = st.data_editor(
                amount_df,
                use_container_width=True,
            )

            # turn df into dict for join_pool
            amount = {
                row["denom"]: row["amount"]
                for _, row in updated_amount_df.iterrows()
                if row["amount"] > 0
            }

            if st.button("Join pool"):
                if st.session_state.simulation.join_pool(amount):
                    timestamp = st.session_state.simulation.latest_timestamp()

                    for denom, amount in amount.items():
                        st.toast(
                            f"Joined pool: `{amount} {denom}` @ `{timestamp}`", icon="🔥"
                        )
                else:
                    st.error("🤦 Join pool failed ")

        elif action == "exit_pool":
            # create editable dataframe for amount
            amount_df = pd.DataFrame(
                columns=["denom", "amount"],
                data=[
                    [denom, 0.0] for denom in st.session_state.simulation.pool.denoms()
                ],
            )

            updated_amount_df = st.data_editor(
                amount_df,
                use_container_width=True,
            )

            # turn df into dict for exit_pool
            amount = {
                row["denom"]: row["amount"]
                for _, row in updated_amount_df.iterrows()
                if row["amount"] > 0
            }

            if st.button("Exit pool"):
                if st.session_state.simulation.exit_pool(amount):
                    timestamp = st.session_state.simulation.latest_timestamp()

                    for denom, amount in amount.items():
                        st.toast(
                            f"Exited pool: `{amount} {denom}` @ `{timestamp}`", icon="🔥"
                        )
                else:
                    st.error("🤦 Exit pool failed ")

        elif action == "swap":
            # select denom_in and denom_out from denoms
            denom_in = st.selectbox(
                "denom_in", st.session_state.simulation.pool.denoms(), index=0
            )

            denom_out = st.selectbox(
                "denom_out", st.session_state.simulation.pool.denoms(), index=1
            )

            # create number input for amount
            amount = st.number_input(
                "amount",
                min_value=0.0,
                max_value=min(
                    st.session_state.simulation.pool.assets[denom_in],
                    st.session_state.simulation.pool.assets[denom_out],
                ),
                value=0.0,
            )

            if st.button("Swap"):
                if st.session_state.simulation.swap(denom_in, denom_out, amount):
                    timestamp = st.session_state.simulation.latest_timestamp()
                    st.toast(
                        f"Swapped: `{amount} {denom_in}` for `{denom_out}` @ `{timestamp}`",
                        icon="🔥",
                    )
                else:
                    st.error("🤦 Swap failed")


snapshots = st.session_state.simulation.snapshots
if snapshots.empty:
    st.info("👈  Start your simulation via __️️️ Control Panel__ on the left sidebar")
else:
    with st.expander("Show raw simulation snapshots"):
        st.dataframe(snapshots, use_container_width=True)

    # latest amount and weight for each denom
    latest = snapshots.groupby("denom").last()[["amount", "weight"]]

    st.write(latest)

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
