//! LCSC-native part sourcing (docs/decisions/0004).
//!
//! Everything here is anonymous — no keys, no login. The load-bearing
//! split: [`fetch_part`] does network I/O and returns a [`RawPart`] of
//! unparsed upstream payloads; conversion to KiCad is pure and lives in
//! [`kicad`]/`convert`, so every conversion test runs from checked-in JSON
//! fixtures with zero network.
//!
//! HTTP policy (verified live 2026-08-08, see the decision doc):
//! - Honest User-Agent. The literal `Mozilla/5.0` is WAF-blocklisted;
//!   `etchable/x.y` passes.
//! - Component data goes `searchByNumbers` -> `GET /api/components/{uuid}`.
//!   The `/api/products/{C#}/components` route every community tool uses is
//!   CloudFront-banned after ~15 requests — never add it back (a test greps
//!   for it).
//! - 403 means "banned for a while", not "retry": a 30-minute circuit
//!   breaker persists to disk.

pub mod cache;
pub mod convert;
pub mod easyeda;
pub mod http;
pub mod jlc;
pub mod kicad;
pub mod part;

pub use cache::Cache;
pub use convert::{convert, ConvertOptions, ConvertedAssets};
pub use http::{Client, FetchError};
pub use part::{fetch_part, FetchOptions, RawPart};

/// CI and tests set `ETCHABLE_LCSC_OFFLINE=1`; every network call then fails
/// fast with [`FetchError::Offline`] instead of touching the wire.
pub fn offline() -> bool {
    std::env::var("ETCHABLE_LCSC_OFFLINE").is_ok_and(|v| v == "1")
}
