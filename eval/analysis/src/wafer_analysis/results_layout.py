"""Portable, fail-closed storage layout for evaluation evidence."""

from __future__ import annotations

import errno
import hashlib
import json
import os
import re
import tempfile
import unicodedata
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

_SAFE_SEGMENT = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
_RESERVED = {"CON", "PRN", "AUX", "NUL", *(f"COM{i}" for i in range(1, 10)), *(f"LPT{i}" for i in range(1, 10))}
CANONICAL_ALIASES = {
    "e-perf-2": "e-perf-1",
    "e-perf-8": "e-perf-6",
    "e-swap-2": "e-swap-1",
    "e-swap-6": "e-swap-1",
}


def validate_segment(value: str) -> str:
    if not _SAFE_SEGMENT.fullmatch(value) or value.endswith((".", " ")):
        raise ValueError(f"path segment is not exFAT-safe: {value!r}")
    if value.split(".", 1)[0].upper() in _RESERVED:
        raise ValueError(f"path segment uses a reserved name: {value!r}")
    return value


def _reject_symlink_chain(path: Path) -> None:
    candidate = path.absolute()
    for part in (candidate, *candidate.parents):
        if part.is_symlink():
            raise ValueError(f"managed path must not use symlinks: {part}")


def _child(root: Path, *parts: str) -> Path:
    path = root
    for raw in parts:
        relative = Path(raw)
        if relative.is_absolute() or not relative.parts:
            raise ValueError(f"managed path must be relative: {raw!r}")
        for part in relative.parts:
            if part in {".", ".."}:
                raise ValueError(f"managed path traversal is forbidden: {raw!r}")
            validate_segment(part)
            if path.is_dir():
                key = unicodedata.normalize("NFC", part).casefold()
                collisions = [
                    child.name
                    for child in path.iterdir()
                    if unicodedata.normalize("NFC", child.name).casefold() == key
                    and child.name != part
                ]
                if collisions:
                    raise ValueError(
                        f"case-only or normalization collision: {collisions[0]!r} and {part!r}"
                    )
            path /= part
    resolved_parent = path.parent.resolve(strict=False)
    if root.resolve(strict=False) not in (resolved_parent, *resolved_parent.parents):
        raise ValueError(f"managed path escapes its root: {path}")
    _reject_symlink_chain(path)
    return path


@dataclass(frozen=True)
class ResultsLayout:
    volume: Path
    raw: Path
    manifests: Path
    derived: Path
    reports: Path
    explicit: bool

    @classmethod
    def resolve(
        cls,
        repo_root: Path,
        results_root: Path | str | None = None,
        *,
        require_mount: bool = True,
        mount_check: Callable[[str | bytes | os.PathLike[str] | os.PathLike[bytes]], bool] | None = None,
    ) -> "ResultsLayout":
        repo = repo_root.resolve()
        configured = results_root if results_root is not None else os.environ.get("WAFER_RESULTS_ROOT")
        if configured is None:
            eval_root = repo / "eval"
            local_results = eval_root / "results"
            return cls(
                volume=eval_root,
                raw=local_results,
                manifests=local_results,
                derived=eval_root / "derived",
                reports=eval_root / "reports",
                explicit=False,
            )

        volume = Path(configured).expanduser()
        if not volume.is_absolute():
            volume = (Path.cwd() / volume).absolute()
        _reject_symlink_chain(volume)
        if not volume.is_dir():
            raise FileNotFoundError(f"explicit results root is absent or not a directory: {volume}")
        volume = volume.resolve()
        is_mount = mount_check or os.path.ismount
        if require_mount and not is_mount(volume):
            raise ValueError(f"explicit results root is not a mounted filesystem: {volume}")
        layout = cls(
            volume=volume,
            raw=volume / "raw",
            manifests=volume / "manifests",
            derived=volume / "derived",
            reports=volume / "reports",
            explicit=True,
        )
        layout.validate()
        return layout

    def validate(self) -> None:
        if not self.explicit:
            return
        roots = (self.raw, self.manifests, self.derived, self.reports)
        if self.volume.is_dir():
            validate_unique_names(list(self.volume.iterdir()))
            for path in roots:
                _child(self.volume, path.name)
        resolved = [path.resolve(strict=False) for path in roots]
        if len(set(resolved)) != len(resolved):
            raise ValueError("raw, manifests, derived, and reports must not overlap")
        for index, left in enumerate(resolved):
            for right in resolved[index + 1 :]:
                if left in right.parents or right in left.parents:
                    raise ValueError("raw, manifests, derived, and reports must not overlap")
        for path in roots:
            _reject_symlink_chain(path)

    def prepare(self) -> None:
        self.validate()
        for path in set((self.raw, self.manifests, self.derived, self.reports)):
            path.mkdir(parents=True, exist_ok=True)
            _reject_symlink_chain(path)

    def raw_path(self, *parts: str) -> Path:
        return _child(self.raw, *parts)

    def manifest_path(self, *parts: str) -> Path:
        return _child(self.manifests, *parts)

    def analysis_path(self, kind: str, *parts: str) -> Path:
        if kind not in {"derived", "reports"}:
            raise ValueError("analysis output kind must be derived or reports")
        return _child(getattr(self, kind), *parts)

    def relative(self, path: Path) -> str:
        resolved = path.resolve(strict=False)
        base = self.volume.resolve() if self.explicit else self.volume.parent.resolve()
        if base not in (resolved, *resolved.parents):
            raise ValueError(f"path is outside the results volume: {path}")
        return resolved.relative_to(base).as_posix()

    def resolve_raw_relative(self, relative: str) -> Path:
        base = self.volume if self.explicit else self.volume.parent
        path = _child(base, relative)
        resolved = path.resolve(strict=False)
        raw = self.raw.resolve(strict=False)
        if raw not in (resolved, *resolved.parents):
            raise ValueError(f"raw reference escapes raw evidence: {relative}")
        return path


def resolve_alias_receipt(receipt_path: Path) -> tuple[dict, Path]:
    if receipt_path.is_symlink():
        raise ValueError(f"alias receipt must not be a symlink: {receipt_path}")
    receipt_path = receipt_path.resolve()
    if receipt_path.stat().st_nlink > 1:
        raise ValueError(f"alias receipt must not be hardlinked: {receipt_path}")
    alias_root = next(
        (parent for parent in receipt_path.parents if parent.name == "aliases"), None
    )
    if alias_root is None:
        raise ValueError(f"alias receipt is not beneath an aliases directory: {receipt_path}")
    manifests = alias_root.parent
    if manifests.name == "manifests":
        volume = manifests.parent
        layout = ResultsLayout(
            volume=volume,
            raw=volume / "raw",
            manifests=manifests,
            derived=volume / "derived",
            reports=volume / "reports",
            explicit=True,
        )
    elif manifests.name == "results" and manifests.parent.name == "eval":
        eval_root = manifests.parent
        layout = ResultsLayout(
            volume=eval_root,
            raw=manifests,
            manifests=manifests,
            derived=eval_root / "derived",
            reports=eval_root / "reports",
            explicit=False,
        )
    else:
        raise ValueError(f"alias receipt is not beneath a managed manifest root: {receipt_path}")
    layout.validate()
    try:
        value = json.loads(receipt_path.read_text())
    except (OSError, ValueError) as error:
        raise ValueError(f"malformed alias receipt: {error}") from error
    if not isinstance(value, dict) or value.get("schema_version") != 1:
        raise ValueError(f"malformed alias receipt: {receipt_path}")
    relative_receipt = receipt_path.relative_to(layout.manifests)
    parts = relative_receipt.parts
    run_match = re.fullmatch(r"run-(\d+)\.json", parts[-1]) if len(parts) >= 5 else None
    condition = "/".join(parts[3:-1]) if len(parts) >= 5 else ""
    if (
        parts[0] != "aliases"
        or value.get("experiment") != parts[1]
        or value.get("condition") != condition
        or run_match is None
        or value.get("run_index") != int(run_match.group(1))
        or value.get("shared_measurement") is not True
        or CANONICAL_ALIASES.get(value.get("experiment"))
        != value.get("shared_from_experiment")
    ):
        raise ValueError(f"alias identity differs from canonical mapping: {receipt_path}")
    source = layout.resolve_raw_relative(str(value.get("source_leaf", "")))
    if not source.is_dir() or source.is_symlink():
        raise ValueError(f"alias source is missing or linked: {source}")
    source_relative = source.relative_to(layout.raw)
    if (
        len(source_relative.parts) < 4
        or source_relative.parts[0] != value["shared_from_experiment"]
        or source_relative.parts[1] != parts[2]
        or "/".join(source_relative.parts[2:-1]) != condition
        or re.fullmatch(
            rf"run-{value['run_index']:02d}-attempt-\d+", source_relative.name
        )
        is None
    ):
        raise ValueError(f"alias source identity differs: {source}")
    status = source / "canonical-status.json"
    try:
        digest = hashlib.sha256(status.read_bytes()).hexdigest()
    except OSError as error:
        raise ValueError(f"alias source has no terminal receipt: {source}") from error
    if value.get("source_status_sha256") != digest:
        raise ValueError(f"alias source receipt digest differs: {source}")
    if value.get("sample_identity") != value.get("source_leaf"):
        raise ValueError(f"alias sample identity differs from source: {receipt_path}")
    return value, source


def atomic_write_json(path: Path, value: dict, *, overwrite: bool = False) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    _reject_symlink_chain(path.parent)
    if path.is_symlink():
        raise ValueError(f"managed artifact must not be a symlink: {path}")
    if path.is_file() and path.stat().st_nlink > 1:
        raise ValueError(f"managed artifact must not be hardlinked: {path}")
    if path.exists() and not overwrite:
        raise FileExistsError(f"refusing to overwrite immutable artifact: {path}")
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=path.parent)
    temporary = Path(temporary_name)
    try:
        if os.fstat(descriptor).st_dev != path.parent.stat().st_dev:
            raise OSError(errno.EXDEV, "temporary file is on another filesystem")
        with os.fdopen(descriptor, "w") as stream:
            descriptor = -1
            json.dump(value, stream, indent=2)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        if path.exists() and not overwrite:
            raise FileExistsError(f"refusing to overwrite immutable artifact: {path}")
        try:
            os.replace(temporary, path)
        except OSError as error:
            if error.errno == errno.EXDEV:
                raise OSError(errno.EXDEV, "cross-device rename is forbidden") from error
            raise
    finally:
        if descriptor >= 0:
            os.close(descriptor)
        temporary.unlink(missing_ok=True)


def validate_unique_names(paths: list[Path]) -> None:
    seen: dict[str, Path] = {}
    for path in paths:
        key = unicodedata.normalize("NFC", path.name).casefold()
        sibling_key = f"{path.parent.resolve(strict=False)}\0{key}"
        previous = seen.get(sibling_key)
        if previous is not None and previous.name != path.name:
            raise ValueError(f"case-only or normalization collision: {previous} and {path}")
        seen[sibling_key] = path


def audit_portability(root: Path) -> None:
    paths = sorted(root.rglob("*"))
    validate_unique_names(paths)
    for path in paths:
        if path.is_symlink():
            raise ValueError(f"symlinks are forbidden in managed evidence: {path}")
        validate_segment(path.name)
        if path.is_file() and path.stat().st_nlink > 1:
            raise ValueError(f"hardlinked evidence is forbidden: {path}")
