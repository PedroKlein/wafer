from pathlib import Path

from wafer_analysis.tables import save_table


def test_save_table_writes_csv_and_latex_without_modifying_source_dataframe(
    tmp_path,
) -> None:
    source = __import__("pandas").DataFrame([{"condition": "wafer", "N_runs": 30}])
    before = source.copy(deep=True)
    csv_path, tex_path = save_table(source, "target-load", tmp_path)
    assert csv_path.read_text().startswith("condition,N_runs")
    assert "\\begin{tabular}" in tex_path.read_text()
    assert source.equals(before)

