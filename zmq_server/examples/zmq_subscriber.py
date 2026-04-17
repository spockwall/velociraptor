"""
Interactive ZMQ subscriber for velociraptor orderbook snapshots.

Usage — subscribe to live data:
    python3 orderbook/examples/zmq_subscriber.py \\
        --exchange binance \\
        --symbol BTCUSDT \\
        --type snapshot \\
        --interval 500

    python3 orderbook/examples/zmq_subscriber.py \\
        --exchange binance \\
        --symbol ETHUSDT \\
        --type bba \\
        --interval 100

Usage — add a new orderbook channel at runtime:
    python3 orderbook/examples/zmq_subscriber.py \\
        --add-channel \\
        --exchange binance \\
        --symbol SOLUSDT

Options:
    --exchange     Exchange name (default: binance)
    --symbol       Symbol in uppercase (default: BTCUSDT)
    --type         Data type: snapshot | bba (default: snapshot)
    --interval     Throttle interval in ms (default: 500)
    --add-channel  Request the server to start streaming a new channel, then exit
    --pub          PUB endpoint (default: tcp://localhost:5555)
    --router       ROUTER endpoint (default: tcp://localhost:5556)
"""

import argparse
import json
import sys
import zmq

def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--exchange",    default="binance")
    p.add_argument("--symbol",      default="BTCUSDT")
    p.add_argument("--type",        default="snapshot", choices=["snapshot", "bba"], dest="sub_type")
    p.add_argument("--interval",    default=500, type=int)
    p.add_argument("--add-channel", action="store_true", dest="add_channel",
                   help="Request the server to start streaming a new exchange/symbol channel")
    p.add_argument("--pub",         default="tcp://localhost:5555")
    p.add_argument("--router",      default="tcp://localhost:5556")
    return p.parse_args()

def add_channel(args):
    """Send an add_channel request and wait for the ack, then exit."""
    ctx = zmq.Context()
    dealer = ctx.socket(zmq.DEALER)
    dealer.connect(args.router)

    request = {
        "action":   "add_channel",
        "exchange": args.exchange,
        "symbol":   args.symbol,
    }
    dealer.send_json(request)
    print(f"Sent: {request}")

    ack = dealer.recv_json()
    print(f"Ack:  {ack}")
    if ack.get("status") != "ok":
        print(f"Failed: {ack.get('message')}", file=sys.stderr)
        sys.exit(1)

    dealer.close()
    ctx.term()
    print(f"Channel {args.exchange}:{args.symbol} requested — subscribe to receive data.")

def subscribe(args):
    ctx = zmq.Context()

    # ── Control socket (DEALER) — send subscribe request, receive ack ──────────
    dealer = ctx.socket(zmq.DEALER)
    dealer.connect(args.router)

    request = {
        "action":   "subscribe",
        "exchange": args.exchange,
        "symbol":   args.symbol,
        "type":     args.sub_type,
        "interval": args.interval,
    }
    dealer.send_json(request)
    print(f"Sent: {request}")

    ack = dealer.recv_json()
    print(f"Ack:  {ack}")
    if ack.get("status") != "ok":
        print(f"Subscription failed: {ack.get('message')}", file=sys.stderr)
        sys.exit(1)

    # ── Data socket (SUB) — receive throttled snapshots ────────────────────────
    sub = ctx.socket(zmq.SUB)
    sub.connect(args.pub)
    topic = f"{args.exchange}:{args.symbol}"
    sub.setsockopt(zmq.SUBSCRIBE, topic.encode())

    print(f"\nSubscribed to '{topic}' ({args.sub_type} @ {args.interval}ms)")
    print("-" * 60)

    try:
        while True:
            topic_frame, payload = sub.recv_multipart()
            snap = json.loads(payload)

            if args.sub_type == "snapshot":
                print(
                    f"[{snap['exchange']}:{snap['symbol']}] seq={snap['sequence']}"
                    f"  bid={snap['best_bid']}  ask={snap['best_ask']}"
                    f"  spread={snap.get('spread', 0):.4f}"
                    f"  wmid={snap.get('wmid', 0):.4f}"
                    f"  depth={len(snap.get('bids',[]))}x{len(snap.get('asks',[]))}"
                )
            else:  # bba
                print(
                    f"[{snap['exchange']}:{snap['symbol']}] seq={snap['sequence']}"
                    f"  bid={snap['best_bid']}  ask={snap['best_ask']}"
                    f"  spread={snap.get('spread', 0):.4f}"
                )
    except KeyboardInterrupt:
        print("\nUnsubscribing...")
        dealer.send_json({
            "action":   "unsubscribe",
            "exchange": args.exchange,
            "symbol":   args.symbol,
            "type":     args.sub_type,
            "interval": args.interval,
        })
    finally:
        dealer.close()
        sub.close()
        ctx.term()

if __name__ == "__main__":
    args = parse_args()
    if args.add_channel:
        add_channel(args)
    else:
        subscribe(args)
