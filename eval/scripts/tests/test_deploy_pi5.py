from __future__ import annotations

import os
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
DEPLOY = ROOT / "eval/scripts/deploy-pi5.sh"


def test_deployed_canonical_runner_starts_from_a_fresh_root(tmp_path: Path) -> None:
    bin_dir = tmp_path / "bin"
    deployed = tmp_path / "deployed"
    bin_dir.mkdir()

    (bin_dir / "ssh").write_text("#!/bin/sh\nexit 0\n")
    (bin_dir / "rsync").write_text(
        "#!/bin/sh\n"
        'source=${2%/}\n'
        'mkdir -p "$DEPLOY_CAPTURE"\n'
        'cp -R "$source/." "$DEPLOY_CAPTURE/"\n'
    )
    for command in ("ssh", "rsync"):
        (bin_dir / command).chmod(0o755)

    env = {
        **os.environ,
        "PATH": f"{bin_dir}:{os.environ['PATH']}",
        "DEPLOY_CAPTURE": str(deployed),
    }
    subprocess.run(
        [str(DEPLOY), "--host", "test@example", "--root", "wafer-fresh"],
        cwd=ROOT,
        env=env,
        check=True,
        capture_output=True,
        text=True,
    )

    assert not any(path.name == "__pycache__" for path in deployed.rglob("*"))
    assert not any(path.suffix == ".pyc" for path in deployed.rglob("*"))
    runner_env = {**os.environ, "PYTHONDONTWRITEBYTECODE": "1"}
    result = subprocess.run(
        ["python3", str(deployed / "eval/scripts/lib/canonical_runner.py"), "--help"],
        cwd=deployed,
        env=runner_env,
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert "Run resumable canonical Pi 5 evaluations" in result.stdout

    analysis_env = {**os.environ, "PYTHONDONTWRITEBYTECODE": "1"}
    analysis = subprocess.run(
        ["uv", "run", "python", "-m", "wafer_analysis.expanded_n5", "--help"],
        cwd=deployed / "eval/analysis",
        env=analysis_env,
        capture_output=True,
        text=True,
        check=False,
    )
    assert analysis.returncode == 0, analysis.stderr
    assert "usage: expanded_n5.py" in analysis.stdout
    assert "--handoff-receipt" in analysis.stdout
    assert "--analyzer-tag" in analysis.stdout
    assert (deployed / "eval/scripts/verify-storage-receipt.py").is_file()
