"""Allows `python -m scripts.trading_engine` to start the engine."""

import sys

from .app import main

if __name__ == "__main__":
    sys.exit(main())
