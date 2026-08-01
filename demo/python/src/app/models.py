"""Plain data, imported by everything and importing nothing but the stdlib."""

from dataclasses import dataclass, field
from datetime import datetime


@dataclass
class Order:
    id: str
    total_cents: int
    placed_at: datetime = field(default_factory=datetime.now)
