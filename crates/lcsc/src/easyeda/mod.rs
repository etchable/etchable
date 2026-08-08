//! EasyEDA component API + document format. `api` does I/O; the document
//! parsers (`doc`, `records`, `units`) are pure and fixture-tested.

pub mod api;
pub mod doc;
pub mod records;
pub mod units;
