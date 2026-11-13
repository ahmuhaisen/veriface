//! Veriface HTTP service (binary `veriface`).
//!
//! - [`api`]: router, middleware and handlers for `contracts/openapi.yaml`
//! - [`engine`]: the seam to the vision pipeline ([`engine::Engine`], [`engine::StubEngine`])
//! - [`pipeline`]: the real engine ([`pipeline::PipelineEngine`])
//! - [`dto`]: response shapes
//! - [`config`], [`profiles`], [`auth`]: startup inputs
//! - [`limiter`], [`metrics`], [`logging`], [`problem`]: runtime concerns
//! - [`net`], [`tls`]: the accept loop (connection limit, header timeout) and in-process TLS
//! - [`server`]: startup sequence and serve loop
//! - [`reload`]: hot reload of the API keys and profiles
//! - [`healthcheck`]: the `healthcheck` command (an HTTP probe for images without curl)
//! - `service` (Windows only): Service Control Manager integration

pub mod api;
pub mod auth;
pub mod config;
pub mod dto;
pub mod engine;
pub mod healthcheck;
pub mod limiter;
pub mod logging;
pub mod metrics;
pub mod net;
pub mod pipeline;
pub mod problem;
pub mod profiles;
pub mod reload;
pub mod server;
#[cfg(windows)]
pub mod service;
pub mod tls;
