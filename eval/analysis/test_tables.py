import pandas as pd

from wafer_analysis.tables import save_table


def test_save_table_writes_csv_and_latex_without_modifying_source_dataframe(
    tmp_path,
) -> None:
    source = pd.DataFrame([{"condition": "wafer", "N_runs": 30}])
    before = source.copy(deep=True)
    csv_path, tex_path = save_table(source, "target-load", tmp_path)
    assert csv_path.read_text().startswith("condition,N_runs")
    assert "\\begin{tabular}" in tex_path.read_text()
    assert source.equals(before)


def test_save_table_formats_numbers_and_keeps_descriptive_columns_out_of_latex(tmp_path) -> None:
    source = pd.DataFrame(
        [
            {
                "condition": "fuel_only",
                "N_runs": 30,
                "median_p99_ns": 50_017_471.0,
                "ratio": 0.012345,
                "missing": float("nan"),
                "gate_passed": True,
                "units": "nanoseconds",
                "estimator": "median of 30 run p99 values",
                "claim_boundary": "harness check",
                "thesis_evidence": True,
            }
        ]
    )
    csv_path, tex_path = save_table(source, "rq1-gate", tmp_path)
    tex = tex_path.read_text()
    assert "fuel\\_only & 30 & 50,017,471 & 0.0123 & -- & yes \\\\" in tex
    assert "claim" not in tex and "estimator" not in tex
    assert "claim_boundary" in csv_path.read_text()


def test_save_table_captions_labels_and_notes_the_table(tmp_path) -> None:
    source = pd.DataFrame(
        [{"condition": "wafer", "N_runs": 30, "units": "nanoseconds", "estimator": "median & 95% CI"}]
    )
    _, tex_path = save_table(source, "rq1-gate", tmp_path, caption="Validation gate (50 ms)")
    tex = tex_path.read_text()
    assert tex.startswith("\\begin{table}[htbp]\n\\centering\n\\caption{Validation gate (50 ms)}\n\\label{tab:rq1-gate}\n\\begin{tabular}")
    assert "Units: nanoseconds. Estimator: median \\& 95\\% CI. Source: the author (2026)." in tex
    assert tex.endswith("\\end{table}\n")


def test_save_table_folds_intervals_into_their_estimate_and_keeps_chosen_columns(tmp_path) -> None:
    source = pd.DataFrame(
        [
            {"condition": "wafer", "extra": 1, "median_p95_ns": 120_000.0, "p95_ci95_low_ns": 110_000.0, "p95_ci95_high_ns": 131_500.0, "pooled_loss": float("nan"), "pooled_loss_ci95_low": 0.0, "pooled_loss_ci95_high": 0.0, "units": "nanoseconds"},
        ]
    )
    _, tex_path = save_table(source, "t", tmp_path, columns=["condition", "median_p95_ns", "p95_ci95_low_ns", "p95_ci95_high_ns", "pooled_loss", "pooled_loss_ci95_low", "pooled_loss_ci95_high"])
    tex = tex_path.read_text()
    assert "condition & median p95 ns & pooled loss \\\\" in tex
    assert "wafer & 120,000 [110,000, 131,500] & -- \\\\" in tex
    assert "extra" not in tex


def test_save_table_leaves_an_interval_apart_when_it_names_another_estimate(tmp_path) -> None:
    source = pd.DataFrame(
        [
            {"median_p95_ns": 2.0, "median_p99_ns": 3.0, "ci95_low_ns": 1.5, "ci95_high_ns": 2.5, "median_ns": 5.0, "ci95_low": 4.0, "ci95_high": 6.0, "p95_sink_gap_ns": 9.0, "bootstrap_median_ci95_low_ns": 7.0, "bootstrap_median_ci95_high_ns": 8.0},
        ]
    )
    _, tex_path = save_table(source, "t", tmp_path)
    tex = tex_path.read_text()
    assert "median p99 ns & ci95 low ns & ci95 high ns & median ns & p95 sink gap ns & bootstrap median ci95 low ns" in tex
    assert "& 3 & 1.5 & 2.5 & 5 [4, 6] & 9 & 7 & 8" in tex
