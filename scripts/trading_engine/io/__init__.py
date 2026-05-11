"""Transport / external I/O.

Anything that crosses a process boundary lives here:

  - `executor_client`: low-level ZMQ DEALER client + msgpack helpers for the
    executor's order ROUTER.
  - `order_router`: thin, strategy-friendly wrapper over `executor_client`.
  - `user_feed`: ZMQ SUB on the user PUB; dispatches fills + order_updates
    to a callback.
"""

from .executor_client import (
    ExecutorClient,
    cancel,
    cancel_all,
    cancel_market,
    heartbeat,
    is_ok,
    place,
    unwrap,
    unwrap_err,
)
from .order_router import OrderRouter
from .user_feed import EventHandler, UserFeed

__all__ = [
    "ExecutorClient",
    "EventHandler",
    "OrderRouter",
    "UserFeed",
    "cancel",
    "cancel_all",
    "cancel_market",
    "heartbeat",
    "is_ok",
    "place",
    "unwrap",
    "unwrap_err",
]
