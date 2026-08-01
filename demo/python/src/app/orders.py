"""Order handling — one arm of the planted cycle.

PLANTED: `app.orders` imports `app.billing`, which imports `app.orders` straight
back. Python allows this at import time and fails only when one of the two is
imported first and finds the other half-initialised, which is why it survives code
review and shows up in production as an AttributeError on a module.
"""

from app.billing import charge

from .models import Order


def place(order: Order) -> str:
    charge(order)
    return order.id
