#!/usr/bin/env python3
"""
Fetch Polymarket token (asset) IDs from the public CLOB API.

Usage:
    # List active markets (default 20)
    python3 scripts/fetch_polymarket_tokens.py

    # Search by keyword
    python3 scripts/fetch_polymarket_tokens.py --search trump

    # Show more results
    python3 scripts/fetch_polymarket_tokens.py --search bitcoin --limit 50

    # Show only token IDs (for piping into the server)
    python3 scripts/fetch_polymarket_tokens.py --search trump --ids-only
"""

import argparse
import json
import sys
import urllib.request
import urllib.parse


CLOB_API = "https://clob.polymarket.com"
GAMMA_API = "https://gamma-api.polymarket.com"


def fetch_markets(search: str | None, limit: int) -> list[dict]:
    """
    Fetch open markets via the Gamma API (supports search + closed filter).
    Returns markets sorted by 24h volume descending.
    """
    return _search_via_gamma(search, limit)


def _search_via_gamma(keyword: str | None, limit: int) -> list[dict]:
    """Fetch open markets via Gamma API. Optionally filter by keyword."""
    params = {"closed": "false", "limit": str(limit)}
    if keyword:
        params["search"] = keyword
    url = f"{GAMMA_API}/markets?" + urllib.parse.urlencode(params)
    req = urllib.request.Request(url, headers={"User-Agent": "velociraptor/1.0"})
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            gamma_markets = json.load(resp)
    except Exception as e:
        print(f"Error fetching from Gamma API: {e}", file=sys.stderr)
        sys.exit(1)

    if not isinstance(gamma_markets, list):
        gamma_markets = gamma_markets.get("data", [])

    # Gamma market has clobTokenIds and outcomes as JSON strings
    results = []
    for m in gamma_markets:
        raw_token_ids = m.get("clobTokenIds", "[]")
        raw_outcomes = m.get("outcomes", "[]")
        try:
            token_ids = json.loads(raw_token_ids) if isinstance(raw_token_ids, str) else raw_token_ids
        except Exception:
            token_ids = []
        try:
            outcomes = json.loads(raw_outcomes) if isinstance(raw_outcomes, str) else raw_outcomes
        except Exception:
            outcomes = []
        tokens = [
            {"token_id": tid, "outcome": outcomes[i] if i < len(outcomes) else str(i)}
            for i, tid in enumerate(token_ids)
        ]
        results.append({
            "question": m.get("question", ""),
            "condition_id": m.get("conditionId") or m.get("condition_id", ""),
            "active": m.get("active", False),
            "closed": m.get("closed", True),
            "volume_24h": float(m.get("volume24hr") or 0),
            "end_date": m.get("endDateIso") or m.get("endDate", ""),
            "tokens": tokens,
        })
    return results


def main():
    parser = argparse.ArgumentParser(description="Fetch Polymarket token IDs")
    parser.add_argument("--search", "-s", help="Filter markets by keyword (case-insensitive)")
    parser.add_argument("--limit", "-n", type=int, default=20, help="Max markets to fetch (default: 20)")
    parser.add_argument(
        "--ids-only",
        action="store_true",
        help="Print only token IDs, one per line (useful for scripting)",
    )
    args = parser.parse_args()

    markets = fetch_markets(args.search, args.limit)

    if not markets:
        print("No markets found.", file=sys.stderr)
        sys.exit(0)

    if args.ids_only:
        for m in markets:
            for token in m.get("tokens", []):
                print(token["token_id"])
        return

    # Sort by 24h volume descending so the most active markets appear first
    markets.sort(key=lambda m: m.get("volume_24h", 0), reverse=True)

    for m in markets:
        question = m.get("question", "(no question)")[:72]
        condition_id = m.get("condition_id", "")
        tokens = m.get("tokens", [])
        closed = m.get("closed", False)
        vol = m.get("volume_24h", 0)
        end_date = m.get("end_date", "")[:10]
        status = "CLOSED" if closed else "open"

        print(f"\n{'─' * 76}")
        print(f"  {question}")
        print(f"  condition_id : {condition_id}")
        print(f"  status       : {status}  |  vol_24h: ${vol:,.0f}  |  ends: {end_date}")
        for t in tokens:
            outcome = t.get("outcome", "?")
            token_id = t.get("token_id", "?")
            print(f"  [{outcome:>4}] token_id: {token_id}")

    print(f"\n{'─' * 76}")
    print(f"  {len(markets)} market(s) shown")
    print()
    print("Use a token_id with the server:")
    print("  cargo run --bin orderbook_server -- --polymarket <token_id>")
    print("  cargo run --example polymarket_orderbook -- --asset <token_id>")


if __name__ == "__main__":
    main()
