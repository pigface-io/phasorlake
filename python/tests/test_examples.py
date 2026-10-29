"""Every public function of the package shows itself in a runnable example.

The doctests run the examples (`--doctest-modules`); this holds each public
function to having one.
"""

from __future__ import annotations

import doctest
import inspect

import pytest

import phasorlake as pl

PUBLIC = [name for name in pl.__all__ if inspect.isfunction(getattr(pl, name))]


def test_the_package_has_public_functions() -> None:
    assert PUBLIC == ["decode", "inspect", "run", "table"]


@pytest.mark.parametrize("name", PUBLIC)
def test_a_public_function_has_an_example(name: str) -> None:
    (test,) = doctest.DocTestFinder(recurse=False).find(getattr(pl, name), name)
    assert test.examples, f"{name} has no example in its docstring"
