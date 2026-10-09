//! `RoomObject`: one Durable Object per meeting, holding its sockets through the WebSocket
//! Hibernation API. All room state lives in SQLite or in socket attachments, never in fields,
//! so the object behaves the same after being evicted and woken by the next message.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use worker::{
    DurableObject, Env, Request, Response, Result, SqlStorage, State, WebSocket,
    WebSocketIncomingMessage, WebSocketPair, WebSocketRequestResponsePair, console_warn,
    durable_object,
};

use crate::ids::ParticipantId;
use crate::protocol::{ClientMsg, ErrorReason, PING, PONG, ServerMsg};

/// How long an empty room waits for someone to come back before it deletes its state.
const EMPTY_ROOM_GRACE: Duration = Duration::from_secs(120);

const SCHEMA: &str =
    "CREATE TABLE IF NOT EXISTS participants (id TEXT PRIMARY KEY, name TEXT NOT NULL)";

/// Per-socket data kept by the runtime across hibernation (limit: 16 KiB serialized).
#[derive(Serialize, Deserialize)]
struct Attachment {
    participant_id: ParticipantId,
}

#[durable_object]
pub struct RoomObject {
    state: State,
}

impl DurableObject for RoomObject {
    fn new(state: State, _env: Env) -> Self {
        let ping_pong =
            WebSocketRequestResponsePair::new(PING, PONG).expect("PING and PONG are plain strings");
        state.set_websocket_auto_response(&ping_pong);
        Self { state }
    }

    /// The router forwards only WebSocket upgrades for this meeting here.
    async fn fetch(&self, _req: Request) -> Result<Response> {
        let participant_id = ParticipantId::random()?;
        // `alarm` drops the table together with all other storage, so recreate it on every join:
        // any socket that can send a message joined after the last wipe.
        self.sql().exec(SCHEMA, None)?;
        let WebSocketPair { client, server } = WebSocketPair::new()?;
        self.state.accept_web_socket(&server);
        server.serialize_attachment(Attachment { participant_id })?;
        Response::from_websocket(client)
    }

    async fn websocket_message(
        &self,
        ws: WebSocket,
        message: WebSocketIncomingMessage,
    ) -> Result<()> {
        let parsed = match message {
            WebSocketIncomingMessage::String(text) => serde_json::from_str::<ClientMsg>(&text).ok(),
            WebSocketIncomingMessage::Binary(_) => None,
        };
        let Some(msg) = parsed else {
            return ws.send(&ServerMsg::Error {
                reason: ErrorReason::Malformed,
            });
        };
        let id = participant_id(&ws)?;
        match msg {
            ClientMsg::Hello { name } => {
                self.sql().exec(
                    "INSERT INTO participants (id, name) VALUES (?, ?) \
                     ON CONFLICT (id) DO UPDATE SET name = excluded.name",
                    vec![id.as_str().into(), name.into()],
                )?;
                Ok(())
            }
            ClientMsg::Chat { text } => match self.name_of(&id)? {
                Some(from) => {
                    self.broadcast(&ServerMsg::Chat {
                        from: &from,
                        text: &text,
                    });
                    Ok(())
                }
                None => ws.send(&ServerMsg::Error {
                    reason: ErrorReason::NotJoined,
                }),
            },
            ClientMsg::Ping => ws.send(&ServerMsg::Pong),
        }
    }

    // Completes the close handshake. Deployed Workers with compatibility date >= 2026-04-07 reply
    // to Close frames themselves (`web_socket_auto_reply_to_close`; calling `close` stays safe),
    // but local `wrangler dev` 4.147.0 does not, and clients then wait for the Close frame forever.
    async fn websocket_close(
        &self,
        ws: WebSocket,
        _code: usize,
        _reason: String,
        _was_clean: bool,
    ) -> Result<()> {
        self.leave(&ws).await?;
        ws.close(Some(1000), Some("bye"))
    }

    async fn websocket_error(&self, ws: WebSocket, _error: worker::Error) -> Result<()> {
        self.leave(&ws).await
    }

    async fn alarm(&self) -> Result<Response> {
        if self.state.get_websockets().is_empty() {
            // Also removes the SQL tables, and the alarm (compatibility date >= 2026-02-24).
            self.state.storage().delete_all().await?;
        }
        Response::empty()
    }
}

impl RoomObject {
    fn sql(&self) -> SqlStorage {
        self.state.storage().sql()
    }

    fn name_of(&self, id: &ParticipantId) -> Result<Option<String>> {
        #[derive(Deserialize)]
        struct Row {
            name: String,
        }
        let rows: Vec<Row> = self
            .sql()
            .exec(
                "SELECT name FROM participants WHERE id = ?",
                vec![id.as_str().into()],
            )?
            .to_array()?;
        Ok(rows.into_iter().next().map(|row| row.name))
    }

    fn broadcast(&self, msg: &ServerMsg<'_>) {
        for socket in self.state.get_websockets() {
            // One socket closing mid-broadcast must not stop delivery to the others.
            if let Err(e) = socket.send(msg) {
                console_warn!("broadcast: send failed: {e}");
            }
        }
    }

    async fn leave(&self, ws: &WebSocket) -> Result<()> {
        let id = participant_id(ws)?;
        self.sql().exec(
            "DELETE FROM participants WHERE id = ?",
            vec![id.as_str().into()],
        )?;
        // Compare by participant id: whether the runtime still lists the closing socket here is
        // undocumented.
        let others_remain = self
            .state
            .get_websockets()
            .iter()
            .any(|socket| participant_id(socket).ok().as_ref() != Some(&id));
        if !others_remain {
            self.state.storage().set_alarm(EMPTY_ROOM_GRACE).await?;
        }
        Ok(())
    }
}

fn participant_id(ws: &WebSocket) -> Result<ParticipantId> {
    ws.deserialize_attachment::<Attachment>()?
        .map(|attachment| attachment.participant_id)
        .ok_or_else(|| "socket has no attachment".into())
}
