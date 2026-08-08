//! Types and codec for the Claude Code CLI stream-json protocol
//! (`claude -p --input-format stream-json --output-format stream-json`).
//!
//! The protocol has no stability promise and drifts across CLI releases, so
//! parsing is deliberately tolerant: every inbound line is first read as raw
//! JSON, known shapes are lifted into typed structs, and anything else is
//! preserved as [`AgentEvent::Unknown`] rather than dropped. Keep all
//! protocol knowledge inside this crate.

mod codec;
mod event;
mod outbound;

pub use codec::StreamJsonCodec;
pub use event::*;
pub use outbound::*;
