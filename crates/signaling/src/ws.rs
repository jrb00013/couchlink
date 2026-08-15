use crate::session::SessionStore;
use axum::extract::ws::{Message, WebSocket};
use couchlink_proto::{Role, SignalMessage};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Relay a player's message to the host. The slot in the message is the
/// connection's *registered* slot — a client-supplied slot is never trusted,
/// so one player can't spoof messages for another player's slot.
fn relay_to_host(store: &SessionStore, session_id: &str, msg: &SignalMessage) {
    if let Some(host_tx) = store.peer_tx(session_id, Role::Host) {
        if let Ok(json) = msg.to_json() {
            let _ = host_tx.send(json);
        }
    }
}

/// Broadcast the current occupancy (`PlayersStatus`) to the host and every
/// connected player, so each client can render "N/3 players connected".
fn broadcast_status(store: &SessionStore, session_id: &str) {
    if let Some((occupied, max)) = store.players_status(session_id) {
        let msg = SignalMessage::PlayersStatus { occupied, max }
            .to_json()
            .unwrap_or_default();
        store.broadcast(session_id, &msg);
    }
}

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
    let mut player_slot: Option<u8> = None;

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
                        session_id: sid.clone(),
                        slot: 0,
                    }
                    .to_json()
                    .unwrap(),
                );
                // A reconnecting host may be coming back to players already seated.
                broadcast_status(&store, &sid);
                info!("host registered for session {}", session_id.as_deref().unwrap_or("?"));
            }
            SignalMessage::RegisterPlayer {
                session_id: sid,
                pin,
                player_name: _,
            } => {
                match store.register_player(sid.clone(), pin, tx.clone()) {
                    Ok((slot, epoch)) => {
                        session_id = Some(sid.clone());
                        role = Some(Role::Player);
                        player_slot = Some(slot);
                        let _ = tx.send(
                            SignalMessage::Registered {
                                role: Role::Player,
                                session_id: sid.clone(),
                                slot,
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
                                            epoch,
                                            slot,
                                        }
                                        .to_json()
                                        .unwrap(),
                                    )
                                    .is_ok();
                                if delivered {
                                    info!("player joined session {sid} (slot {slot}, epoch {epoch})");
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
                        broadcast_status(&store, &sid);
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
            // --- player → host: stamp the connection's own slot, then relay ---
            SignalMessage::Answer { sdp, epoch, .. } => {
                if let (Some(sid), Some(slot)) = (session_id.as_deref(), player_slot) {
                    relay_to_host(&store, sid, &SignalMessage::Answer { sdp, epoch, slot });
                }
            }
            SignalMessage::IceCandidate {
                candidate,
                sdp_mid,
                sdp_mline_index,
                slot,
            } => match role {
                // player → host: stamp the connection's own slot, never trust the payload.
                Some(Role::Player) => {
                    if let (Some(sid), Some(my_slot)) = (session_id.as_deref(), player_slot) {
                        relay_to_host(
                            &store,
                            sid,
                            &SignalMessage::IceCandidate {
                                candidate,
                                sdp_mid,
                                sdp_mline_index,
                                slot: my_slot,
                            },
                        );
                    }
                }
                // host → player: route by the slot the host stamped.
                Some(Role::Host) => {
                    if let Some(sid) = session_id.as_deref() {
                        match store.player_tx(sid, slot) {
                            Some(player_tx) => {
                                let _ = player_tx.send(text.clone());
                            }
                            None => {
                                warn!("ice candidate for unknown slot {slot} dropped (session {sid})")
                            }
                        }
                    }
                }
                None => {}
            },
            SignalMessage::PadInfo { kind, id, .. } => {
                if let (Some(sid), Some(slot)) = (session_id.as_deref(), player_slot) {
                    relay_to_host(&store, sid, &SignalMessage::PadInfo { kind: kind.clone(), id: id.clone(), slot });
                    // Every player also gets to see it — a controller debug
                    // view needs every seated player's pad, not just its own.
                    if let Ok(json) = (SignalMessage::PlayerPadInfo { slot, kind, id }).to_json() {
                        store.broadcast(sid, &json);
                    }
                }
            }
            SignalMessage::PresentPath { path, .. } => {
                if let (Some(sid), Some(slot)) = (session_id.as_deref(), player_slot) {
                    relay_to_host(&store, sid, &SignalMessage::PresentPath { path, slot });
                }
            }
            SignalMessage::RequestOffer { .. } => {
                if let (Some(sid), Some(slot)) = (session_id.as_deref(), player_slot) {
                    relay_to_host(&store, sid, &SignalMessage::RequestOffer { slot });
                }
            }
            // --- host → player: route by the slot the host stamped ---
            SignalMessage::Offer { slot, .. } => {
                if let (Some(sid), Some(Role::Host)) = (session_id.as_deref(), role) {
                    match store.player_tx(sid, slot) {
                        Some(player_tx) => {
                            let _ = player_tx.send(text.clone());
                        }
                        None => warn!("offer for unknown slot {slot} dropped (session {sid})"),
                    }
                }
            }
            // Shared stream telemetry fans out to every connected player.
            SignalMessage::StreamInfo { .. } | SignalMessage::HostStats { .. } => {
                if let (Some(sid), Some(Role::Host)) = (session_id.as_deref(), role) {
                    store.broadcast_to_players(sid, &text);
                }
            }
            _ => {}
        }
    }

    if let (Some(sid), Some(r)) = (session_id, role) {
        let was_player = r == Role::Player;
        store.unregister(&sid, r, Some(&tx));
        if was_player {
            broadcast_status(&store, &sid);
        }
    }
    store.dec_conn();
    forward.abort();
}
