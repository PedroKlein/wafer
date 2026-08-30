from __future__ import annotations

import pathlib

import pandas as pd

from canonical_power import render


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
