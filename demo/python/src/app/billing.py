"""Billing — the other arm of the planted cycle.

Imports `app.orders` for a type it could have taken from `app.models`, which is the
ordinary way one of these appears.
"""

from app.orders import place

from .models import Order


def charge(order: Order) -> int:
    return order.total_cents


def replay(order: Order) -> str:
    return place(order)
