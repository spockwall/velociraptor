//! Backend HTTP API — axum service that answers the React frontend by
//! reading Redis (and disk spillover for paged history).
//!
//! Also publishes `CONTROL_SOCKET` ZMQ messages for shutdown / strategy
//! params. Phase 6 contains the full implementation.
