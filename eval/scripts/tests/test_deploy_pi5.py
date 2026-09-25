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
    release_dir = ROOT / "target/docker-aarch64-linux/release"
    created_dirs = [
        path
        for path in (ROOT / "target", release_dir.parent, release_dir)
        if not path.exists()
    ]
    release_dir.mkdir(parents=True, exist_ok=True)
    created_binaries = []
    for name in ("wafer", "wafer-loadgen", "waferctl"):
        binary = release_dir / name
        if not binary.exists():
            binary.write_text("#!/bin/sh\nexit 0\n")
            binary.chmod(0o755)
            created_binaries.append(binary)
    try:
        subprocess.run(
            [str(DEPLOY), "--host", "test@example", "--root", "wafer-fresh"],
            cwd=ROOT,
            env=env,
            check=True,
            capture_output=True,
            text=True,
        )
    finally:
        for binary in created_binaries:
            binary.unlink()
        for directory in reversed(created_dirs):
            directory.rmdir()

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
