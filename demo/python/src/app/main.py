"""The entrypoint.

VIOLATED: tropism.toml says the entrypoint goes through the api and nothing else.
This module reaches straight into storage.
"""

import sys

from app.api import fetch_rate
from app.storage import load_schema


def main() -> int:
    schema = load_schema()
    print(f"{len(schema)} keys, rate {fetch_rate('GBP')}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
