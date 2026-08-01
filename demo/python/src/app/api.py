"""The HTTP surface.

TRAP: `from .models import Order` is a relative import that must resolve to the
module `app.models`, not to an undeclared package called `models`.

PLANTED: httpx is imported and never declared in pyproject.toml.
"""

import json

import httpx
import requests

from .models import Order


def fetch_rate(currency: str) -> float:
    response = requests.get(f"https://example.invalid/rates/{currency}", timeout=5)
    return json.loads(response.text)["rate"]


async def notify(order: Order) -> None:
    async with httpx.AsyncClient() as client:
        await client.post("https://example.invalid/notify", json={"id": order.id})
