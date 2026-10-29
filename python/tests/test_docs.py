"""Every Python example in docs/python.md runs, from the repository root."""

from __future__ import annotations

import re

import pytest

from .conftest import ROOT

DOC = ROOT / "docs" / "python.md"
BLOCKS = re.findall(r"```python\n(.*?)```", DOC.read_text(), re.DOTALL)


@pytest.mark.parametrize("block", BLOCKS, ids=[b.splitlines()[0] for b in BLOCKS])
def test_example_runs(block: str, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.chdir(ROOT)
    namespace: dict[str, object] = {}
    exec(compile(block, str(DOC), "exec"), namespace)  # ruff: ignore[exec-builtin] — the documentation's own examples


def test_the_document_has_examples() -> None:
    assert len(BLOCKS) >= 3
