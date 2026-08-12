use crate::session::SessionStore;
use axum::extract::ws::{Message, WebSocket};
use couchlink_proto::{Role, SignalMessage};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

pub async fn handle_socket(socket: WebSocket, store: Arc<SessionStore>) {
    store.inc_conn();
    let (sender, mut receiver) = socket.split();
    let sender = Arc::new(Mutex::new(sender));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let sender_fwd = Arc::clone(&sender);
    let forward = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let mut s = sender_fwd.lock().await;
            if s.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    let mut session_id: Option<String> = None;
    let mut role: Option<Role> = None;

    while let Some(Ok(msg)) = receiver.next().await {
        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Ping(p) => {
                let mut s = sender.lock().await;
                let _ = s.send(Message::Pong(p)).await;
                continue;
            }
            Message::Close(_) => break,
            _ => continue,
        };

        let parsed = match SignalMessage::from_json(&text) {
            Ok(m) => m,
            Err(e) => {
                let _ = tx.send(
                    SignalMessage::Error {
                        message: format!("invalid message: {e}"),
                    }
                    .to_json()
                    .unwrap(),
                );
                continue;
            }
        };

        match parsed {
            SignalMessage::RegisterHost {
                session_id: sid,
                pin,
                device_name,
                preset,
                emulator,
            } => {
                if let Err(e) = store.register_host(
                    sid.clone(),
                    pin,
                    device_name,
                    preset,
                    emulator,
                    tx.clone(),
                ) {
                    let _ = tx.send(
                        SignalMessage::Error { message: e }.to_json().unwrap(),
                    );
                    continue;
                }
                session_id = Some(sid.clone());
                role = Some(Role::Host);
                let _ = tx.send(
                    SignalMessage::Registered {
                        role: Role::Host,
                        session_id: sid,
                    }
                    .to_json()
                    .unwrap(),
                );
                info!("host registered for session {}", session_id.as_deref().unwrap_or("?"));
            }
            SignalMessage::RegisterPlayer {
                session_id: sid,
                pin,
                player_name: _,
            } => {
                match store.register_player(sid.clone(), pin, tx.clone()) {
                    Ok(player_epoch) => {
                        session_id = Some(sid.clone());
                        role = Some(Role::Player);
                        let _ = tx.send(
                            SignalMessage::Registered {
                                role: Role::Player,
                                session_id: sid.clone(),
                            }
                            .to_json()
                            .unwrap(),
                        );
                        // Always notify the host: a reload leaves a stale player tx
                        // behind, and suppressing PeerJoined would strand the browser
                        // waiting for an offer that never comes.
                        match store.peer_tx(&sid, Role::Host) {
                            Some(host_tx) => {
                                let delivered = host_tx
                                    .send(
                                        SignalMessage::PeerJoined {
                                            role: Role::Player,
                                            epoch: player_epoch,
                                        }
                                        .to_json()
                                        .unwrap(),
                                    )
                                    .is_ok();
                                if delivered {
                                    info!("player joined session {sid} (epoch {player_epoch})");
                                } else {
                                    // The slot holds a sender whose receiver is gone,
                                    // so the host socket is already dead and nobody
                                    // will ever answer this player.
                                    warn!(
                                        "player joined session {sid} but the host channel \
                                         is closed — the host will not be told"
                                    );
                                }
                            }
                            None => warn!(
                                "player joined session {sid} with no host registered — \
                                 it will wait for an offer that cannot come"
                            ),
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(
                            SignalMessage::Error { message: e }.to_json().unwrap(),
                        );
                    }
                }
            }
            SignalMessage::Heartbeat => {
                if let Some(sid) = &session_id {
                    store.touch(sid);
                }
                let _ = tx.send(SignalMessage::Pong.to_json().unwrap());
            }
            SignalMessage::Pong => {
                if let Some(sid) = &session_id {
                    store.touch(sid);
                }
            }
            SignalMessage::RequestOffer => {
                if let (Some(sid), Some(r)) = (&session_id, role) {
                    store.relay(sid, r, &text);
                }
            }
            SignalMessage::Offer { .. }
            | SignalMessage::Answer { .. }
            | SignalMessage::IceCandidate { .. }
            | SignalMessage::StreamInfo { .. }
            | SignalMessage::HostStats { .. }
            | SignalMessage::PadInfo { .. }
            | SignalMessage::PresentPath { .. } => {
                if let (Some(sid), Some(r)) = (&session_id, role) {
                    store.relay(sid, r, &text);
                }
            }
            _ => {}
        }
    }

    if let (Some(sid), Some(r)) = (session_id, role) {
        store.unregister(&sid, r, Some(&tx));
    }
    store.dec_conn();
    forward.abort();
}
