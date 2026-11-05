//! Linear for HumanLayer Local, using a personal API key:
//! - [`client::Client`]: the few GraphQL calls the app needs.
//! - [`ticket`]: parse ticket references, format `ticket.md`, find images.
//! - [`status`]: which workflow state a board stage moves a ticket to
//!   (forward only, ported from upstream `linear-status-progression.ts`).

pub mod client;
pub mod status;
pub mod ticket;

pub use client::{Client, Comment, Issue, State};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Linear rejected the API key. Check it in Settings.")]
    Auth,
    #[error("Linear ticket {0} was not found, or this API key can't see it.")]
    NotFound(String),
    #[error("Could not reach Linear: {0}")]
    Network(String),
    #[error("Linear error: {0}")]
    Api(String),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;
