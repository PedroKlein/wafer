from __future__ import annotations

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np
from matplotlib.collections import LineCollection, PathCollection

from wafer_analysis.plots import strip_with_ci


def test_strip_shows_every_run_its_median_and_its_interval() -> None:
    fig, ax = plt.subplots()
    groups = {"WAFER": np.arange(30.0) * 1_000, "eKuiper": np.full(30, 50_000.0)}
    strip_with_ci(
        ax,
        groups,
        colors={"WAFER": "#c2410c", "eKuiper": "#6d28d9"},
        intervals={"WAFER": (10_000.0, 20_000.0)},
        scale=1e3,
    )
    dots = [collection for collection in ax.collections if isinstance(collection, PathCollection)]
    lines = [collection for collection in ax.collections if isinstance(collection, LineCollection)]
    assert sum(len(collection.get_offsets()) for collection in dots) == 60
    medians = [segment[0][1] for collection in lines for segment in collection.get_segments() if segment[0][1] == segment[1][1]]
    assert sorted(medians) == [14.5, 50.0]
    whiskers = [segment for collection in lines for segment in collection.get_segments() if segment[0][0] == segment[1][0]]
    assert [(whisker[0][1], whisker[1][1]) for whisker in whiskers] == [(10.0, 20.0)]
    assert [label.get_text() for label in ax.get_xticklabels()] == ["WAFER", "eKuiper"]
    plt.close(fig)
