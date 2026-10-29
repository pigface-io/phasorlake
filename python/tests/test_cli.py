"""The console scripts run the bundled binaries."""

from __future__ import annotations

import os
import subprocess  # ruff: ignore[suspicious-subprocess-import] — the tests run the installed scripts
import sys
from typing import TYPE_CHECKING

import pytest

import phasorlake as pl
from phasorlake import _cli

if TYPE_CHECKING:
    from pathlib import Path


def test_run_is_the_bundled_phasor() -> None:
    assert pl.run(["--version"]) == 0
    assert pl.run(["--version"], binary_name="phasor-mirror") == 0
    assert pl.run(["no-such-command"]) != 0


def test_the_package_imports_without_pyarrow() -> None:
    # The console scripts import the package; pyarrow would add its import to every command.
    code = "import sys, phasorlake; sys.exit('pyarrow' in sys.modules)"
    subprocess.run([sys.executable, "-c", code], check=True)  # ruff: ignore[subprocess-without-shell-equals-true] — this interpreter


def test_the_console_scripts_are_on_the_path() -> None:
    for name in ("phasor", "phasor-mirror"):
        out = subprocess.run([name, "--version"], check=True, capture_output=True, text=True)  # ruff: ignore[subprocess-without-shell-equals-true] — the scripts this package installs
        assert out.stdout.startswith(f"{name} ")


def test_a_missing_binary_says_how_to_build_it(tmp_path: Path) -> None:
    with pytest.raises(FileNotFoundError, match="just py-bin"):
        _cli.binary("phasor", tmp_path)


def test_phasor_replaces_the_process(monkeypatch: pytest.MonkeyPatch) -> None:
    seen: list[tuple[str, list[str]]] = []

    def execv(path: os.PathLike[str], argv: list[str]) -> None:
        seen.append((str(path), argv))
        raise SystemExit(0)

    monkeypatch.setattr(os, "execv", execv)
    monkeypatch.setattr(sys, "argv", ["phasor", "list", "--json"])
    with pytest.raises(SystemExit):
        _cli.phasor()
    ((path, argv),) = seen
    assert path.endswith("/bin/phasor")
    assert argv == [path, "list", "--json"]


def test_phasor_mirror_replaces_the_process(monkeypatch: pytest.MonkeyPatch) -> None:
    seen: list[str] = []

    def execv(path: os.PathLike[str], argv: list[str]) -> None:
        seen.append(str(path))
        seen.extend(argv[1:])
        raise SystemExit(0)

    monkeypatch.setattr(os, "execv", execv)
    monkeypatch.setattr(sys, "argv", ["phasor-mirror", "status"])
    with pytest.raises(SystemExit):
        _cli.phasor_mirror()
    assert seen[0].endswith("/bin/phasor-mirror")
    assert seen[1:] == ["status"]


def test_on_windows_the_script_runs_the_exe_and_leaves_with_its_code(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    (tmp_path / "phasor.exe").write_bytes(b"")
    monkeypatch.setattr(os, "name", "nt")
    monkeypatch.setattr(_cli, "_BIN", tmp_path)
    monkeypatch.setattr(sys, "argv", ["phasor", "--version"])

    def run(argv: list[str], *, check: bool) -> subprocess.CompletedProcess[bytes]:
        assert not check
        assert argv == [str(tmp_path / "phasor.exe"), "--version"]
        return subprocess.CompletedProcess(argv, 3)

    monkeypatch.setattr(subprocess, "run", run)
    with pytest.raises(SystemExit) as e:
        _cli.phasor()
    assert e.value.code == 3
