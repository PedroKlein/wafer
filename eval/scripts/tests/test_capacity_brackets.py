from __future__ import annotations

import copy
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "eval/analysis/src/wafer_analysis"))

from capacity_brackets import derive_bracket_rates, frozen_bracket_rates  # noqa: E402

MATRIX = json.loads((ROOT / "eval/canonical-matrix.json").read_text())


def resolved(lower: int, upper: int) -> dict:
    return {"phase": "resolved", "lower_good_rate_msg_s": lower, "upper_bad_rate_msg_s": upper}


def scout(**states: dict) -> dict:
    return {
        "action": "stop",
        "reason": "all-suts-resolved-or-support-censored",
        "states": {
            "mqtt-loopback": resolved(15_500, 16_000),
            "native": resolved(12_000, 13_000),
            "wafer": resolved(10_000, 11_000),
            "ekuiper": resolved(6_000, 6_500),
            **states,
        },
    }


def test_rates_bracket_both_ceilings_and_hit_both_threshold_points() -> None:
    rates = derive_bracket_rates(scout(), MATRIX)

    assert rates == [4_500, 5_700, 6_300, 9_500, 10_500, 15_100]
    # WAFER delivering 4,500 and eKuiper failing 6,300 is a PASS on its own.
    assert 4_500 / 6_300 >= 0.70
    # WAFER failing 10,500 and eKuiper delivering 15,100 is a FAIL on its own.
    assert 10_500 / 15_100 < 0.70


def test_a_point_exactly_on_the_threshold_stays_on_the_deciding_side() -> None:
    rates = derive_bracket_rates(scout(ekuiper=resolved(9_500, 10_000)), MATRIX)

    assert {7_000, 10_000} <= set(rates)
    assert 7_000 / 10_000 >= 0.70
    assert 15_000 not in rates and 15_100 in rates


def test_the_threshold_comes_from_the_declared_verdict_rule() -> None:
    matrix = copy.deepcopy(MATRIX)
    row = next(
        row
        for row in matrix["verdict_rules"]["thresholds"]
        if row["criterion"] == "e-perf-10-competitive-ratio"
    )
    row["value"] = 0.8

    assert derive_bracket_rates(scout(), matrix) == [5_100, 5_700, 6_300, 9_500, 10_500, 13_200]


def test_common_grid_rates_are_not_added_but_still_anchor_a_threshold_point() -> None:
    rates = derive_bracket_rates(scout(ekuiper=resolved(3_800, 4_000)), MATRIX)

    assert 4_000 not in rates
    assert {3_600, 2_800} <= set(rates)


def test_rates_above_the_support_path_are_dropped_with_their_threshold_point() -> None:
    rates = derive_bracket_rates(
        scout(**{"mqtt-loopback": resolved(10_200, 10_800)}), MATRIX
    )

    assert rates == [4_500, 5_700, 6_300, 9_500]


def test_an_unsaturated_support_path_caps_at_its_last_doubling() -> None:
    rates = derive_bracket_rates(
        scout(
            **{"mqtt-loopback": {"phase": "geometric", "rate_msg_s": 32_000}},
            wafer=resolved(20_000, 22_000),
        ),
        MATRIX,
    )

    assert rates == [4_500, 5_700, 6_300]


def test_censored_scout_states_add_no_rates() -> None:
    summary = scout(
        wafer={"phase": "support-censored", "censor_above_rate_msg_s": 15_500},
        ekuiper={"phase": "left-censored", "rate_msg_s": 500},
    )

    assert derive_bracket_rates(summary, MATRIX) == []


def test_extra_rates_stop_at_the_declared_maximum() -> None:
    matrix = copy.deepcopy(MATRIX)
    matrix["final_campaign"]["capacity_grid"]["bracket_rates"]["max_extra_rates"] = 3

    assert derive_bracket_rates(scout(), matrix) == [5_700, 9_500, 10_500]


@pytest.mark.parametrize(
    ("summary", "message"),
    [
        ({**scout(), "action": "launch"}, "not from a finished scout"),
        ({"action": "stop"}, "not from a finished scout"),
        (scout(wafer={"phase": "refine", "rate_msg_s": 9_000}), "no finished wafer state"),
        (scout(ekuiper=None), "no finished ekuiper state"),
        (
            scout(**{"mqtt-loopback": {"phase": "left-censored", "rate_msg_s": 500}}),
            "no delivery-good MQTT-loopback rate",
        ),
        (scout(wafer=resolved(0, 500)), "not a positive integer rate"),
    ],
)
def test_an_unusable_scout_summary_is_refused(summary: dict, message: str) -> None:
    with pytest.raises(ValueError, match=message):
        derive_bracket_rates(summary, MATRIX)


def test_frozen_rates_are_read_back_only_when_they_fit_the_rule() -> None:
    assert frozen_bracket_rates({"capacity_brackets": {"rates_msg_s": [5_700, 6_300]}}, MATRIX) == [
        5_700,
        6_300,
    ]
    assert frozen_bracket_rates({"capacity_brackets": {"rates_msg_s": []}}, MATRIX) == []
    for rates in (
        [6_300, 5_700],
        [5_700, 5_700],
        [4_000],
        [5_700.0],
        [0],
        [100, 200, 300, 400, 500, 600, 700],
        None,
    ):
        with pytest.raises(ValueError, match="no valid E-Perf-10 bracket rates"):
            frozen_bracket_rates({"capacity_brackets": {"rates_msg_s": rates}}, MATRIX)
    with pytest.raises(ValueError, match="no valid E-Perf-10 bracket rates"):
        frozen_bracket_rates({}, MATRIX)
