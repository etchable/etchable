//! NDJSON codec: frames lines, parses inbound [`AgentEvent`]s, encodes
//! [`Outbound`] messages. Malformed JSON lines become `Unknown` events with
//! the raw line attached instead of killing the stream — the CLI sometimes
//! interleaves non-protocol output on error paths.

use bytes::BytesMut;
use serde_json::json;
use tokio_util::codec::{Decoder, Encoder, LinesCodec, LinesCodecError};

use crate::{AgentEvent, Outbound};

pub struct StreamJsonCodec {
    lines: LinesCodec,
}

impl Default for StreamJsonCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamJsonCodec {
    pub fn new() -> Self {
        Self {
            // Schematic JSON in tool results can be large; cap lines at 64 MiB
            // rather than the LinesCodec default of unlimited-but-unbounded.
            lines: LinesCodec::new_with_max_length(64 * 1024 * 1024),
        }
    }

    fn parse_line(line: String) -> AgentEvent {
        match serde_json::from_str(&line) {
            Ok(value) => AgentEvent::from_json(value),
            Err(_) => AgentEvent::Unknown(json!({"type": "raw_line", "line": line})),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("line framing error: {0}")]
    Lines(#[from] LinesCodecError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialize error: {0}")]
    Serialize(#[from] serde_json::Error),
}

impl Decoder for StreamJsonCodec {
    type Item = AgentEvent;
    type Error = CodecError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        loop {
            match self.lines.decode(src)? {
                Some(line) if line.trim().is_empty() => continue,
                Some(line) => return Ok(Some(Self::parse_line(line))),
                None => return Ok(None),
            }
        }
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        loop {
            match self.lines.decode_eof(src)? {
                Some(line) if line.trim().is_empty() => continue,
                Some(line) => return Ok(Some(Self::parse_line(line))),
                None => return Ok(None),
            }
        }
    }
}

impl Encoder<Outbound> for StreamJsonCodec {
    type Error = CodecError;

    fn encode(&mut self, item: Outbound, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let line = item.to_json_line()?;
        self.lines.encode(line, dst)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_multiple_lines_and_skips_blanks() {
        let mut codec = StreamJsonCodec::new();
        let mut buf = BytesMut::from(
            "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s\"}\n\n{\"type\":\"result\",\"subtype\":\"success\"}\n",
        );
        assert!(matches!(
            codec.decode(&mut buf).unwrap(),
            Some(AgentEvent::System(_))
        ));
        assert!(matches!(
            codec.decode(&mut buf).unwrap(),
            Some(AgentEvent::Result(_))
        ));
        assert!(codec.decode(&mut buf).unwrap().is_none());
    }

    #[test]
    fn garbage_lines_survive_as_unknown() {
        let mut codec = StreamJsonCodec::new();
        let mut buf = BytesMut::from("not json at all\n");
        match codec.decode(&mut buf).unwrap() {
            Some(AgentEvent::Unknown(v)) => assert_eq!(v["line"], "not json at all"),
            other => panic!("wrong: {other:?}"),
        }
    }

    #[test]
    fn encodes_newline_terminated() {
        let mut codec = StreamJsonCodec::new();
        let mut buf = BytesMut::new();
        codec
            .encode(Outbound::user_text("hi", None), &mut buf)
            .unwrap();
        let s = String::from_utf8(buf.to_vec()).unwrap();
        assert!(s.ends_with('\n'));
        assert_eq!(s.matches('\n').count(), 1);
    }
}
