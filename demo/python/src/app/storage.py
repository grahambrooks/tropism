"""Persistence.

TRAP: `import yaml` comes from the PyYAML distribution. The import name and the
package name share no characters in common, and tropism must resolve one to the other
rather than reporting PyYAML unused *and* yaml missing.

VIOLATED: `requests` is restricted to the api layer by tropism.toml, and this module
uses it to fetch a schema at import time.
"""

import os

import requests
import yaml

SCHEMA_URL = os.environ.get("SCHEMA_URL", "https://example.invalid/schema.yaml")


def load_schema() -> dict:
    response = requests.get(SCHEMA_URL, timeout=5)
    return yaml.safe_load(response.text)
