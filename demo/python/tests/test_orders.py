"""TRAP: a test module importing the code under test is not a cycle, and `pytest`
is a dev dependency that is used here and nowhere else."""

import pytest

from app.models import Order
from app.orders import place


@pytest.mark.parametrize("total", [0, 1000])
def test_place_returns_the_id(total: int) -> None:
    assert place(Order(id="x", total_cents=total)) == "x"
