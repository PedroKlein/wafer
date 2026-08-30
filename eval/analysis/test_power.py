from __future__ import annotations

import pathlib

import pandas as pd
import pytest

from canonical_power import read_measurement_window, read_message_count, render
from wafer_analysis.power import clip_to_window, summarize_power


def test_external_subscriber_message_count_ignores_empty_gap_csv(
    tmp_path: pathlib.Path,
) -> None:
    (tmp_path / "sequence.csv").write_text("event_type,seq_start,seq_end,count\n")
    (tmp_path / "subscriber-metadata.json").write_text('{"total_recorded":60000}')
    assert read_message_count(tmp_path) == 60_000


def test_in_process_message_count_excludes_warmup_messages(
    tmp_path: pathlib.Path,
) -> None:
    (tmp_path / "percentiles.json").write_text('{"total_count":60000}')
    (tmp_path / "sequence.csv").write_text(
        "total_expected,total_received,gap_ranges,gap_msgs,duplicates_count\n"
        "90000,90000,0,0,0\n"
    )
    assert read_message_count(tmp_path) == 60_000


def test_measurement_window_excludes_warmup_energy() -> None:
    samples = [
        {
            "timestamp_ns": timestamp * 1_000_000_000,
            "temperature_millicelsius": 50_000,
            "cpu_frequency_hz": 2_400_000_000,
            "governor": "performance",
            "throttled": "0x0",
            "rail_proxy_watts": watts,
        }
        for timestamp, watts in ((0, 20.0), (10, 2.0), (20, 2.0))
    ]
    measured = clip_to_window(samples, 10_000_000_000, 20_000_000_000)
    summary = summarize_power(measured, messages=1_000)
    assert summary["duration_s"] == 10.0
    assert summary["mean_proxy_watts"] == 2.0
    assert summary["proxy_energy_j"] == 20.0
    assert summary["proxy_energy_per_message_j"] == 0.02
    with pytest.raises(ValueError, match="does not overlap"):
        clip_to_window(samples, 30_000_000_000, 40_000_000_000)


def test_measurement_window_is_required(tmp_path: pathlib.Path) -> None:
    with pytest.raises(ValueError, match="missing measurement window"):
        read_measurement_window(tmp_path)
    (tmp_path / "measurement-window.json").write_text(
        '{"started_ns":100,"finished_ns":200}'
    )
    assert read_measurement_window(tmp_path) == (100, 200)


def test_power_report_writes_data_and_vector_figure(tmp_path: pathlib.Path) -> None:
    data = pd.DataFrame(
        [
            {
                "experiment": "e-perf-1",
                "system": "wafer",
                "condition": "wafer",
                "run": "run-01",
                "measurement": "Raspberry Pi 5 PMIC internal-rail proxy",
                "mean_proxy_watts": 4.2,
                "proxy_energy_j": 252.0,
                "proxy_energy_per_message_j": 0.0042,
            },
            {
                "experiment": "e-perf-1",
                "system": "native",
                "condition": "native",
                "run": "run-01",
                "measurement": "Raspberry Pi 5 PMIC internal-rail proxy",
                "mean_proxy_watts": 4.0,
                "proxy_energy_j": 240.0,
                "proxy_energy_per_message_j": 0.0040,
            },
            {
                "experiment": "e-perf-1",
                "system": "ekuiper",
                "condition": "ekuiper",
                "run": "run-01",
                "measurement": "Raspberry Pi 5 PMIC internal-rail proxy",
                "mean_proxy_watts": 4.5,
                "proxy_energy_j": 270.0,
                "proxy_energy_per_message_j": 0.0045,
            },
        ]
    )
    render(data, tmp_path)
    assert (tmp_path / "pmic-energy-summary.csv").is_file()
    assert (tmp_path / "pmic-energy-summary.tex").is_file()
    assert (tmp_path / "power/pmic_proxy.pdf").stat().st_size > 1000
    assert (tmp_path / "power/pmic_proxy.png").stat().st_size > 1000
    assert "PMIC internal-rail proxy" in (tmp_path / "pmic-energy-summary.csv").read_text()
