# roomrs-async

Runtime-agnostic asynchronous facade for [roomrs](https://github.com/yongaru/roomrs). It exposes `Send` futures and live-query stream support, with optional tokio integration.

## Usage

This is an internal implementation crate. Application code should normally enable the `async` feature on [`roomrs`](https://crates.io/crates/roomrs) and use its re-exported API.

See the [repository README](https://github.com/yongaru/roomrs#readme) for async examples and feature details.
