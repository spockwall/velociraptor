"""
Phase 1 smoke test — send a Heartbeat to the executor for Polymarket and
print the OrderResponse. No money at risk.

Run the executor first:

    cargo run -p executor --bin executor -- \
        --credentials credentials/polymarket.yaml \
        --polymarket-env prod \
        --skip-chmod-check

Then in another shell:

    python scripts/poly_heartbeat.py
"""

from __future__ import annotations

import argparse
import json
import sys

from executor_client import ExecutorClient, heartbeat, is_ok


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--endpoint", default="tcp://127.0.0.1:5557")
    ap.add_argument("--exchange", default="polymarket")
    ap.add_argument("--count", type=int, default=3)
    ap.add_argument("--timeout-ms", type=int, default=5_000)
    args = ap.parse_args()

    print(f"connecting DEALER → {args.endpoint}")
    with ExecutorClient(args.endpoint, timeout_ms=args.timeout_ms) as cli:
        all_ok = True
        for i in range(args.count):
            req = heartbeat(args.exchange)
            print(f"[{i + 1}/{args.count}] → {json.dumps(req)}")
            resp = cli.send(req)
            print(f"[{i + 1}/{args.count}] ← {json.dumps(resp, default=str)}")
            if not is_ok(resp):
                all_ok = False
                print(f"  ! result is Err: {resp.get('result')}", file=sys.stderr)
                continue
            if resp.get("req_id") != req["req_id"]:
                all_ok = False
                print(
                    f"  ! req_id mismatch: sent {req['req_id']}, got {resp.get('req_id')}",
                    file=sys.stderr,
                )

        print()
        print("RESULT:", "ok" if all_ok else "FAIL")
        return 0 if all_ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
