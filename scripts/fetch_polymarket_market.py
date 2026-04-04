"""
Fetch Polymarket up/down market info for the current (and optionally upcoming) windows.

The slug timestamp is the Unix epoch of the window END time, aligned to the interval:
    btc-updown-15m-{ts}   — ts is a multiple of 900  (15 min)
    btc-updown-5m-{ts}    — ts is a multiple of 300  (5 min)
    eth-updown-15m-{ts}
    eth-updown-5m-{ts}

This script computes the current window slugs from the system clock and fetches them.

Usage:
    python scripts/fetch_polymarket_market.py              # current window
    python scripts/fetch_polymarket_market.py --next 3     # current + next 3 windows
    python scripts/fetch_polymarket_market.py --slug btc-updown-15m-1775299500  # explicit
"""

import argparse
import json
import sys
import time
import urllib.request
from dataclasses import dataclass

BASE_URL = "https://gamma-api.polymarket.com/markets/slug"
HEADERS = {"Accept": "application/json", "User-Agent": "Mozilla/5.0"}

# (coin, interval_seconds, label)
MARKETS = [
    ("btc", 900, "15m"),
    ("eth", 900, "15m"),
    ("btc", 300, "5m"),
    ("eth", 300, "5m"),
]


@dataclass
class MarketInfo:
    slug: str
    question: str
    active: bool
    closed: bool
    end_date: str
    outcomes: list
    prices: list
    best_bid: str
    best_ask: str
    spread: str
    last_price: str
    liquidity: str
    volume: str
    token_ids: list


def current_window_ts(interval: int) -> int:
    """Return the Unix timestamp of the END of the current window."""
    now = int(time.time())
    # next boundary = ceil to the next multiple of interval
    return ((now // interval) + 1) * interval


def build_slugs(lookahead: int) -> list[str]:
    """Return slugs for the current + next `lookahead` windows for all market types."""
    slugs = []
    for coin, interval, label in MARKETS:
        base_ts = current_window_ts(interval)
        for i in range(lookahead + 1):
            ts = base_ts + i * interval
            slugs.append(f"{coin}-updown-{label}-{ts}")
    return slugs


def fetch_market(slug: str) -> dict | None:
    url = f"{BASE_URL}/{slug}"
    req = urllib.request.Request(url, headers=HEADERS)
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            return json.loads(resp.read())
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return None
        raise


def _parse_json_field(value) -> list:
    """Some fields come back as a JSON-encoded string instead of a list."""
    if isinstance(value, list):
        return value
    if isinstance(value, str):
        return json.loads(value)
    return []


def parse_market(m: dict) -> MarketInfo:
    return MarketInfo(
        slug=m.get("slug", ""),
        question=m.get("question", ""),
        active=m.get("active", False),
        closed=m.get("closed", False),
        end_date=m.get("endDate", ""),
        outcomes=_parse_json_field(m.get("outcomes", [])),
        prices=_parse_json_field(m.get("outcomePrices", [])),
        best_bid=m.get("bestBid", ""),
        best_ask=m.get("bestAsk", ""),
        spread=m.get("spread", ""),
        last_price=m.get("lastTradePrice", ""),
        liquidity=m.get("liquidity", ""),
        volume=m.get("volume", ""),
        token_ids=_parse_json_field(m.get("clobTokenIds", [])),
    )


def print_market(info: MarketInfo) -> None:
    status = "ACTIVE" if info.active and not info.closed else "CLOSED"
    print(f"\n{'─' * 64}")
    print(f"  [{status}] {info.question}")
    print(f"  slug     : {info.slug}")
    print(f"  ends     : {info.end_date}")
    outcomes = info.outcomes
    prices   = info.prices
    for outcome, price in zip(outcomes, prices):
        print(f"  {outcome:<8}: {price}")
    print(f"  bid/ask  : {info.best_bid} / {info.best_ask}  spread: {info.spread}")
    print(f"  last     : {info.last_price}  liquidity: {info.liquidity}  volume: {info.volume}")
    if info.token_ids:
        for i, tid in enumerate(info.token_ids):
            label = outcomes[i] if i < len(outcomes) else str(i)
            print(f"  token[{label}]: {tid}")


def main() -> None:
    parser = argparse.ArgumentParser(description="Fetch Polymarket up/down market info")
    parser.add_argument("--next", type=int, default=0, metavar="N",
                        help="fetch current + next N windows (default: 0 = current only)")
    parser.add_argument("--slug", nargs="+", metavar="SLUG",
                        help="fetch specific slugs instead of auto-computing")
    args = parser.parse_args()

    slugs = args.slug if args.slug else build_slugs(args.next)

    found = 0
    for slug in slugs:
        data = fetch_market(slug)
        if data is None:
            continue
        print_market(parse_market(data))
        found += 1

    if found == 0:
        print("No markets found. The current window may not be open yet.", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
