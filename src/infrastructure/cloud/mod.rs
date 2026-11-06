//! Localify Cloud: the credentials a signed-in machine keeps, the passwords
//! it set, the HTTP client for its API, and the API's types. The contract is
//! `docs/public-urls.md`.

pub mod api;
pub mod credentials;
pub mod http;
pub mod passwords;
pub mod runtime;
pub mod types;
