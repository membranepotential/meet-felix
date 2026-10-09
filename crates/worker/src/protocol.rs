//! Placeholder signalling protocol for spike 1; `meeting_core::ClientMsg`/`ServerMsg` replace it.

use serde::{Deserialize, Serialize};

/// The exact frame the runtime answers with [`PONG`] without waking the room
/// (`setWebSocketAutoResponse` matches the raw string, not the parsed message).
pub const PING: &str = r#"{"type":"ping"}"#;
pub const PONG: &str = r#"{"type":"pong"}"#;

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Hello {
        name: String,
    },
    Chat {
        text: String,
    },
    /// Reaches the room only when spelled differently from [`PING`].
    Ping,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg<'a> {
    Chat { from: &'a str, text: &'a str },
    Pong,
    Error { reason: ErrorReason },
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorReason {
    /// Not a text frame holding a known [`ClientMsg`].
    Malformed,
    /// `Chat` before `Hello`.
    NotJoined,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_response_frames_match_the_protocol() {
        assert_eq!(
            serde_json::from_str::<ClientMsg>(PING).unwrap(),
            ClientMsg::Ping
        );
        assert_eq!(serde_json::to_string(&ServerMsg::Pong).unwrap(), PONG);
    }

    #[test]
    fn chat_wire_format() {
        let msg = ClientMsg::Chat { text: "hi".into() };
        assert_eq!(
            serde_json::from_str::<ClientMsg>(r#"{"type":"chat","text":"hi"}"#).unwrap(),
            msg
        );
        assert_eq!(
            serde_json::to_string(&ServerMsg::Chat {
                from: "Ada",
                text: "hi"
            })
            .unwrap(),
            r#"{"type":"chat","from":"Ada","text":"hi"}"#
        );
    }
}
