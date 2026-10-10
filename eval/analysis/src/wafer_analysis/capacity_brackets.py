"""E-Perf-10 bracket rates: the rates one host adds to the common capacity grid.

``final_campaign.capacity_grid.bracket_rates`` in the matrix declares how a host's capacity
scout summary (``scout-complete.json``) turns into extra E-Perf-10 rates. The runner derives
them when a batch starts and freezes them in ``batch.json`` with the SHA-256 of the summary.

The runner loads this module as a top-level module, so it uses no package-relative imports.
"""

from __future__ import annotations

import math
from collections.abc import Mapping
from fractions import Fraction

COMPETITIVE_PAIR = ("wafer", "ekuiper")
FINISHED_SUT_PHASES = frozenset({"resolved", "left-censored", "support-censored"})


def _rule(matrix: Mapping) -> tuple[Mapping, list[int], Fraction]:
    grid = matrix["final_campaign"]["capacity_grid"]
    rule = grid["bracket_rates"]
    threshold = next(
        row["value"]
        for row in matrix["verdict_rules"]["thresholds"]
        if row["criterion"] == rule["competitive_threshold"]
    )
    # Exact decimal arithmetic keeps a rate exactly on the threshold on the side the rule says.
    return rule, list(grid["common_rate_points_msg_s"]), Fraction(str(threshold))


def _rate(state: Mapping, field: str) -> int:
    value = state.get(field)
    if type(value) is not int or value <= 0:
        raise ValueError(f"scout state {field} is not a positive integer rate")
    return value


def _support_ceiling(state: object) -> int:
    phase = state.get("phase") if isinstance(state, Mapping) else None
    if phase == "resolved":
        return _rate(state, "lower_good_rate_msg_s")
    if phase == "geometric":
        # A finished scout leaves the loopback doubling only after a good probe, so the
        # highest delivered rate is half the next one.
        return _rate(state, "rate_msg_s") // 2
    raise ValueError("the scout found no delivery-good MQTT-loopback rate")


def _host_rates(rule: Mapping, host: str) -> list[int]:
    return list(rule.get("host_rates_msg_s", {}).get(host, []))


def derive_bracket_rates(scout: Mapping, matrix: Mapping, host: str) -> list[int]:
    """The extra E-Perf-10 rates for ``host``: the bracket rule applied to one scout summary,
    plus the fixed rates the matrix declares for that host."""
    rule, common, threshold = _rule(matrix)
    states = scout.get("states")
    if scout.get("action") != "stop" or not isinstance(states, Mapping):
        raise ValueError("the scout summary is not from a finished scout")
    support = _support_ceiling(states.get("mqtt-loopback"))
    step = int(rule["step_msg_s"])
    low, high = (Fraction(str(value)) for value in rule["ceiling_multipliers"])
    brackets = {}
    for system in COMPETITIVE_PAIR:
        state = states.get(system)
        if not isinstance(state, Mapping) or state.get("phase") not in FINISHED_SUT_PHASES:
            raise ValueError(f"the scout summary has no finished {system} state")
        if state["phase"] == "resolved":
            ceiling = _rate(state, "lower_good_rate_msg_s")
            brackets[system] = (ceiling * low // step * step, math.ceil(ceiling * high / step) * step)
    candidates = [rate for pair in brackets.values() for rate in pair]
    if "ekuiper" in brackets and brackets["ekuiper"][1] <= support:
        candidates.append(math.ceil(threshold * brackets["ekuiper"][1] / step) * step)
    if "wafer" in brackets and brackets["wafer"][1] <= support:
        candidates.append((brackets["wafer"][1] / threshold // step + 1) * step)
    rates: list[int] = []
    for rate in candidates:
        if 0 < rate <= support and rate not in common and rate not in rates:
            rates.append(rate)
    rates = rates[: int(rule["max_extra_rates"])]
    rates += [rate for rate in _host_rates(rule, host) if rate not in rates]
    return sorted(rates)


def frozen_bracket_rates(batch: Mapping, matrix: Mapping) -> list[int]:
    """The bracket rates a batch.json froze, once they fit the declared rule."""
    rule, common, _ = _rule(matrix)
    record = batch.get("capacity_brackets")
    rates = record.get("rates_msg_s") if isinstance(record, Mapping) else None
    if (
        not isinstance(rates, list)
        or any(type(rate) is not int or rate <= 0 or rate in common for rate in rates)
        or rates != sorted(set(rates))
        or len(rates) > int(rule["max_extra_rates"]) + len(_host_rates(rule, batch.get("host")))
        or not set(_host_rates(rule, batch.get("host"))) <= set(rates)
    ):
        raise ValueError("batch.json records no valid E-Perf-10 bracket rates")
    return rates
