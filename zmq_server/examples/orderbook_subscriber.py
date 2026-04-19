#!/usr/bin/env python3
"""
Velociraptor orderbook subscriber.

Sends a subscribe request over the DEALER→ROUTER control socket, then
receives snapshots from the PUB socket on every engine update.

Wire format
-----------
Control (DEALER↔ROUTER):  JSON
Data    (SUB←PUB):        msgpack — two frames: [topic_bytes, payload_bytes]

Usage
-----
    # Binance BTC snapshot (default)
    python3 zmq_server/examples/orderbook_subscriber.py

    # OKX ETH, BBA type
    python3 zmq_server/examples/orderbook_subscriber.py \\
        --exchange okx --symbol ETH-USDT-SWAP --type bba

    # Hyperliquid
    python3 zmq_server/examples/orderbook_subscriber.py \\
        --exchange hyperliquid --symbol BTC

    # Polymarket (token ID as symbol)
    python3 zmq_server/examples/orderbook_subscriber.py \\
        --exchange polymarket \\
        --symbol 71321045679252212594626385532706912750332728571942532289631379312455583992563

Options
-------
    --exchange   Exchange name, lowercase  (default: binance)
    --symbol     Symbol as published       (default: BTCUSDT)
    --type       snapshot | bba            (default: snapshot)
    --pub        PUB endpoint              (default: tcp://localhost:5555)
    --router     ROUTER endpoint           (default: tcp://localhost:5556)
"""

import argparse
import sys

try:
    import zmq
except ImportError:
    sys.exit("Missing dependency: pip install pyzmq")

try:
    import msgpack
except ImportError:
    sys.exit("Missing dependency: pip install msgpack")


# ── CLI ───────────────────────────────────────────────────────────────────────

def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--exchange", default="binance")
    p.add_argument("--symbol",   default="BTCUSDT")
    p.add_argument("--type",     default="snapshot", choices=["snapshot", "bba"],
                   dest="sub_type")
    p.add_argument("--pub",    default="tcp://localhost:5555")
    p.add_argument("--router", default="tcp://localhost:5556")
    return p.parse_args()


# ── Decode ────────────────────────────────────────────────────────────────────

def decode_payload(raw: bytes) -> dict:
    # rmp_serde::to_vec_named → msgpack map with str keys.
    return msgpack.unpackb(raw, raw=False)


def exchange_name(val) -> str:
    # ExchangeName serialises as a single-key map e.g. {"Binance": 0}
    if isinstance(val, dict):
        return next(iter(val), "?")
    return str(val)


def fmt_level(val) -> str:
    if val is None:
        return "None"
    if isinstance(val, (list, tuple)) and len(val) == 2:
        return f"{val[0]:.4f} × {val[1]:.4f}"
    return str(val)


# ── Printers ──────────────────────────────────────────────────────────────────

def print_snapshot(msg: dict) -> None:
    ex     = exchange_name(msg.get("exchange"))
    sym    = msg.get("symbol", "?")
    seq    = msg.get("sequence", "?")
    bid    = fmt_level(msg.get("best_bid"))
    ask    = fmt_level(msg.get("best_ask"))
    spread = msg.get("spread")
    wmid   = msg.get("wmid", 0.0)
    bids   = msg.get("bids", [])
    asks   = msg.get("asks", [])
    spread_str = f"{spread:.6f}" if spread is not None else "None"
    wmid_str   = f"{wmid:.6f}"  if isinstance(wmid, (int, float)) else str(wmid)
    print(
        f"[{ex}:{sym}] seq={seq}"
        f"  bid={bid}  ask={ask}"
        f"  spread={spread_str}  wmid={wmid_str}"
        f"  depth={len(bids)}×{len(asks)}"
    )


def print_bba(msg: dict) -> None:
    ex     = exchange_name(msg.get("exchange"))
    sym    = msg.get("symbol", "?")
    seq    = msg.get("sequence", "?")
    bid    = fmt_level(msg.get("best_bid"))
    ask    = fmt_level(msg.get("best_ask"))
    spread = msg.get("spread")
    spread_str = f"{spread:.6f}" if spread is not None else "None"
    print(f"[{ex}:{sym}] seq={seq}  bid={bid}  ask={ask}  spread={spread_str}")


# ── Main ──────────────────────────────────────────────────────────────────────

def main() -> None:
    args = parse_args()
    ctx  = zmq.Context()

    # Control socket — DEALER (no identity envelope needed).
    dealer = ctx.socket(zmq.DEALER)
    dealer.connect(args.router)

    request = {
        "action":   "subscribe",
        "exchange": args.exchange,
        "symbol":   args.symbol,
        "type":     args.sub_type,
    }
    dealer.send_json(request)
    print(f"→ {request}")

    ack = dealer.recv_json()
    print(f"← {ack}")
    if ack.get("status") != "ok":
        print(f"Subscription failed: {ack.get('message')}", file=sys.stderr)
        sys.exit(1)

    # Data socket — SUB filtered to our topic.
    sub = ctx.socket(zmq.SUB)
    sub.connect(args.pub)
    topic = f"{args.exchange}:{args.symbol}"
    sub.setsockopt(zmq.SUBSCRIBE, topic.encode())

    print(f"\nListening on '{topic}' ({args.sub_type})")
    print("-" * 60)

    printer = print_snapshot if args.sub_type == "snapshot" else print_bba

    try:
        while True:
            frames = sub.recv_multipart()
            if len(frames) != 2:
                print(f"[warn] unexpected frame count: {len(frames)}", file=sys.stderr)
                continue
            _topic, payload = frames
            try:
                msg = decode_payload(payload)
            except Exception as e:
                print(f"[warn] decode error: {e}  raw={payload[:80]!r}", file=sys.stderr)
                continue
            printer(msg)

    except KeyboardInterrupt:
        print("\nUnsubscribing…")
        dealer.send_json({
            "action":   "unsubscribe",
            "exchange": args.exchange,
            "symbol":   args.symbol,
        })
    finally:
        dealer.close()
        sub.close()
        ctx.term()


if __name__ == "__main__":
    main()
