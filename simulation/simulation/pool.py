from limiters import Limiter


class Pool:
    def __init__(self, denoms: list[str]):
        self.assets = {denom: 0.0 for denom in denoms}
        self.limiters = {denom: [] for denom in denoms}

    def denoms(self) -> list[str]:
        return list(self.assets.keys())

    def weight(self, denom: str):
        total_assets = sum(self.assets.values())
        if total_assets == 0:
            return 0
        return self.assets[denom] / total_assets

    # =======

    def join_pool(self, timestamp: int, amount: dict[str, float]) -> bool:
        for denom in amount.keys():
            self.assets[denom] += amount[denom]

        ok = not self.surpassed_limit(timestamp)
        
        if not ok:
            for denom in amount.keys():
                self.assets[denom] -= amount[denom]
        
        return ok

    def exit_pool(self, timestamp: int, amount: dict[str, float]) -> bool:
        amount = amount.copy()

        for denom in amount.keys():
            # ensure that assets will not be negative
            if self.assets[denom] < amount[denom]:
                amount[denom] = self.assets[denom]

            self.assets[denom] -= amount[denom]

        ok = not self.surpassed_limit(timestamp)
        
        if not ok:
            for denom in amount.keys():
                self.assets[denom] += amount[denom]
        
        return ok

    def swap(self, denom_in: str, denom_out: str, timestamp: int, amount: float) -> bool:
        self.assets[denom_in] += amount
        self.assets[denom_out] -= amount

        ok = not self.surpassed_limit(timestamp)
        
        if not ok:
            self.assets[denom_in] -= amount
            self.assets[denom_out] += amount
        
        return ok

    def set_limiters(self, denom: str, limiters: list[Limiter]):
        if denom not in self.assets:
            raise ValueError(f"Denom {denom} is not in the pool.")
        self.limiters[denom] = limiters

    def surpassed_limit(self, timestamp: int) -> bool:
        for denom in self.denoms():
            for limiter in self.limiters[denom]:
                if limiter.surpassed_limit(timestamp, self.weight(denom)):
                    return True
        return False

    def add_new_denom(self, denom: str):
        self.assets[denom] = 0
