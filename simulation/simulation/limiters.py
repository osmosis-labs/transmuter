class Limiter:
    def surpassed_limit(self, _timestamp: int, _value: float) -> bool:
        raise NotImplementedError


class StaticLimiter(Limiter):
    def __init__(self, upper_limit: float) -> None:
        self.upper_limit = upper_limit

    def surpassed_limit(self, _timestamp: int, value: float):
        return value > self.upper_limit


# a stateful limiter
class ChangeLimiter(Limiter):
    def __init__(self, offset: float, window_length: int) -> None:
        self.offset = offset

        # window is a list of (time, weight) tuples
        self.window: list[tuple[int, float]] = []
        self.window_length = window_length

    def update(self, timestamp: int, value: float):
        """
        add new value to window
        """
        self.window.append((timestamp, value))
        self.window = self.window[-self.window_length :]

    def surpassed_limit(self, timestamp: int, value: float) -> bool:
        """
        find time weighted moving avarage of window + offset compared to value
        """
        if len(self.window) == 0:
            return False

        # calculate time weighted moving average
        twma = 0
        for i in range(len(self.window) - 1):
            twma += (self.window[i + 1][0] - self.window[i][0]) * self.window[i][1]

        twma = twma / (timestamp - self.window[0][0])

        return value > twma + self.offset
