---
name: velociraptor-python-clients
description: Python (pyzmq + msgpack) recipes for velociraptor — subscribing to market snapshots, reading user events, sending orders to the executor, and pydantic models that mirror the Rust protocol. Use whenever writing a Python client for the ZMQ surface.
---

# Velociraptor — Python Client Recipes

Install: `pip install pyzmq msgpack pydantic`.

For socket/topic/payload reference see `velociraptor-zmq-protocol`.

## Subscribe to market snapshots (DEALER handshake + SUB)

```python
import zmq, msgpack

ctx = zmq.Context()

# 1. Subscribe handshake on DEALER → ROUTER (JSON control)
dealer = ctx.socket(zmq.DEALER)
dealer.connect("tcp://localhost:5556")
dealer.send_json({
    "action": "subscribe", "exchange": "binance",
    "symbol": "BTCUSDT", "type": "snapshot",   # or "bba"
})
ack = dealer.recv_json()
assert ack["status"] == "ok", ack

# 2. Read market PUB
sub = ctx.socket(zmq.SUB)
sub.connect("tcp://localhost:5555")
sub.setsockopt(zmq.SUBSCRIBE, b"binance:BTCUSDT")

while True:
    _topic, payload = sub.recv_multipart()
    snap = msgpack.unpackb(payload, raw=False)
    exchange = next(iter(snap["exchange"]))   # {"binance":0} → "binance"
    print(f"[{exchange}:{snap['symbol']}] "
          f"bid={snap['best_bid']} ask={snap['best_ask']} wmid={snap['wmid']:.4f}")
```

To unsubscribe before closing: `dealer.send_json({"action":"unsubscribe","exchange":"binance","symbol":"BTCUSDT"})`.

Full reference subscriber: `zmq_server/examples/orderbook_subscriber.py` — supports all exchanges and BBA mode.

```bash
python3 zmq_server/examples/orderbook_subscriber.py                                    # Binance BTC snapshot
python3 zmq_server/examples/orderbook_subscriber.py --exchange okx --symbol ETH-USDT-SWAP --type bba
python3 zmq_server/examples/orderbook_subscriber.py --exchange hyperliquid --symbol BTC
python3 zmq_server/examples/orderbook_subscriber.py --exchange kalshi --symbol KXBTC15M-26APR130415-15
python3 zmq_server/examples/orderbook_subscriber.py --exchange polymarket \
    --symbol 71321045679252212594626385532706912750332728571942532289631379312455583992563
```

## Subscribe to user events (no handshake)

```python
import zmq, msgpack

ctx = zmq.Context()
sub = ctx.socket(zmq.SUB)
sub.connect("ipc:///tmp/trading/ws_status.sock")
sub.setsockopt(zmq.SUBSCRIBE, b"user.polymarket.")   # or b"user." for all

while True:
    topic, payload = sub.recv_multipart()
    ev = msgpack.unpackb(payload, raw=False)
    if ev.get("type") == "fill":
        print(f"FILL {ev['side']} {ev['qty']} @ {ev['px']} fee={ev['fee']}")
    elif ev.get("type") == "order_update":
        print(f"ORDER {ev['status']} oid={ev['exchange_oid']}")
```

## Send an order (REQ/REP)

```python
import zmq, msgpack

ctx = zmq.Context()
req = ctx.socket(zmq.REQ)
req.connect("ipc:///tmp/trading/executor_orders.sock")
# or tcp://localhost:5557 in compose

req.send(msgpack.packb({
    "req_id": 1,
    "exchange": "polymarket",
    "action": "place",
    "client_oid": "my-order-001",
    "symbol": "<token_id>",
    "side": "buy",
    "kind": "limit",
    "px": 0.55,
    "qty": 10.0,
    "tif": "GTC",
}, use_bin_type=True))

response = msgpack.unpackb(req.recv(), raw=False)
print(response)   # {"req_id": 1, "result": {"result": "ack", ...}}
```

## Pydantic models (mirror Rust `libs/src/protocol/`)

Keep these in lockstep with the Rust enums:

```python
from typing import Literal
from pydantic import BaseModel

class PlaceOne(BaseModel):
    client_oid: str
    symbol: str
    side: Literal["buy", "sell"]
    kind: Literal["limit", "market"]
    px: float
    qty: float
    tif: Literal["GTC", "IOC", "FOK", "GTD"]

class PlaceAction(PlaceOne):
    action: Literal["place"] = "place"

class OrderRequest(BaseModel):
    req_id: int
    exchange: str
    action: dict   # discriminated by "action" key
```

## Reading recorded MessagePack files

See `velociraptor-storage` for the file format and reader recipes — `data/binance/...`, `data/binance_spot/.../*-trades.mpack`, Polymarket window files, price-to-beat CSVs.
