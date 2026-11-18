//! Integrasi GitHub dan GitLab untuk OnCode.
//!
//! - [`mod@provider`]: satu trait ([`ForgeProvider`]) untuk kedua penyedia.
//! - [`auth`]: login OAuth device flow dan pembaruan token.
//! - [`store`]: penyimpanan token di keychain OS ([`KeychainStore`]) atau memori.
//! - [`model`]: tipe yang dikirim ke UI, dan [`parse_remote_url`].
//!
//! Crate ini tidak bergantung pada Tauri. Token tidak pernah muncul di keluaran `Debug`,
//! pesan galat, maupun log.

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
    parse_remote_url, Account, Check, CheckState, ForgeHost, ForgeKind, NewPullRequest, Page, PrFilter, PrState,
    PullRequest, PullRequestDetail, RemoteRepo, RepoRef, Timestamp,
};
pub use provider::{git_credentials, provider, pull_request_ref, ForgeProvider};
pub use store::{KeychainStore, MemoryStore, TokenStore};
