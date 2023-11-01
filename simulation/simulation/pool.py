from limiters import Limiter


class Pool:
    def __init__(self, denoms: list[str]):
        self.assets = {denom: 0 for denom in denoms}
        self.limiters = {denom: [] for denom in denoms}

    def denoms(self) -> list[str]:
        return list(self.assets.keys())

    def weight(self, denom: str):
        total_assets = sum(self.assets.values())
        if total_assets == 0:
            return 0
        return self.assets[denom] / total_assets

    # =======

    def join_pool(self, timestamp: int, amount: dict[str, float]):
        for denom in amount.keys():
            self.assets[denom] += amount[denom]

        if self.surpassed_limit(timestamp):
            for denom in amount.keys():
                self.assets[denom] -= amount[denom]
            return

    def exit_pool(self, timestamp: int, amount: dict[str, float]):
        amount = amount.copy()

        for denom in amount.keys():
            # ensure that assets will not be negative
            if self.assets[denom] < amount[denom]:
                amount[denom] = self.assets[denom]

            self.assets[denom] -= amount[denom]

        if self.surpassed_limit(timestamp):
            for denom in amount.keys():
                self.assets[denom] += amount[denom]
            return

    def swap(self, denom_in: str, denom_out: str, timestamp: int, amount: float):
        self.assets[denom_in] += amount
        self.assets[denom_out] -= amount

        if self.surpassed_limit(timestamp):
            self.assets[denom_in] -= amount
            self.assets[denom_out] += amount
            return

    def set_limiters(self, denom: str, limiters: list[Limiter]):
        self.limiters[denom] = limiters

    def surpassed_limit(self, timestamp: int) -> bool:
        for denom in self.denoms():
            for limiter in self.limiters[denom]:
                if limiter.surpassed_limit(timestamp, self.weight(denom)):
                    return True
        return False

    # TODO: add new denom and see how simulation perform
    def add_new_denom(denom: str):
        pass
