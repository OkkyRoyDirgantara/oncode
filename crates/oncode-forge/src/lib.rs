//! GitHub and GitLab integration for OnCode.
//!
//! - [`mod@provider`]: one trait ([`ForgeProvider`]) for both providers.
//! - [`auth`]: OAuth device flow login and token refresh.
//! - [`store`]: token storage in the OS keychain ([`KeychainStore`]) or in memory.
//! - [`model`]: types sent to the UI, and [`parse_remote_url`].
//!
//! This crate does not depend on Tauri. Tokens never appear in `Debug` output,
//! error messages, or logs.

pub mod auth;
pub mod error;
mod github;
mod gitlab;
mod http;
pub mod model;
pub mod provider;
mod remote;
pub mod store;

pub use auth::{device_poll, device_start, refresh_token, DeviceCode, DevicePoll, TokenSet};
pub use error::{ForgeError, ForgeErrorCode, Result};
pub use model::{
    parse_remote_url, Account, Check, CheckState, ForgeHost, ForgeKind, MergeInput, MergeMethod, MergeOutcome, MergeState, NewPullRequest, Page, PrFilter, PrState,
    NewReviewComment, PullRequest, PullRequestDetail, RemoteRepo, RepoRef, ReviewComment, ReviewEvent, ReviewInput, ReviewSide, Timestamp,
};
pub use provider::{git_credentials, provider, pull_request_ref, ForgeProvider};
pub use store::{KeychainStore, MemoryStore, TokenStore};
