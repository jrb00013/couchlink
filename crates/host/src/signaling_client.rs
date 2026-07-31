//! Signaling link to the couchlink server, with the socket owned by a supervisor
//! that reconnects and re-registers on its own.
//!
//! The host used to hold a single websocket for its whole life. One transient error
//! anywhere — a dropped frame, a server restart, a network blip — ended the server's
//! read loop, which unregistered the host from the session. The host never noticed:
//! it carried on capturing and encoding perfectly happily, to a session it was no
//! longer part of. Every player that joined after that point registered fine, found
//! no host to notify, and waited forever for an offer nobody was listening to ask
//! for. Observed after three hours of uptime and 599k frames streamed.
//!
//! The outbound sender and inbound receiver handed to the rest of the host are
//! therefore stable across reconnects: only the socket underneath is replaced.

use anyhow::Result;
use couchlink_proto::SignalMessage;
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{info, warn};

/// How long to wait between reconnect attempts. Short enough that a server restart
/// is invisible in practice, long enough not to spin on a server that is down.
const RECONNECT_DELAY: Duration = Duration::from_secs(1);

pub struct SignalingClient {
    pub outbound: mpsc::UnboundedSender<SignalMessage>,
    pub inbound: mpsc::UnboundedReceiver<SignalMessage>,
}

/// Everything needed to re-establish the host's identity after a reconnect.
#[derive(Clone)]
pub struct HostRegistration {
    pub session_id: String,
    pub pin: String,
    pub device_name: String,
    pub preset: String,
    pub emulator: String,
}

impl HostRegistration {
    fn message(&self) -> SignalMessage {
        SignalMessage::RegisterHost {
            session_id: self.session_id.clone(),
            pin: self.pin.clone(),
            device_name: Some(self.device_name.clone()),
            preset: Some(self.preset.clone()),
            emulator: Some(self.emulator.clone()),
        }
    }
}

impl SignalingClient {
    /// Connect, register, and keep both true for the life of the process.
    ///
    /// Returns once the first registration is acknowledged, so callers can rely on
    /// being registered; after that the supervisor maintains it silently.
    pub async fn connect_and_register(url: &str, registration: HostRegistration) -> Result<Self> {
        let (out_tx, out_rx) = mpsc::unbounded_channel::<SignalMessage>();
        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<SignalMessage>();

        let url = url.to_string();
        tokio::spawn(supervise(url, registration, out_rx, in_tx));

        // Wait for the first Registered before handing control back, so a caller that
        // immediately expects to be live is not lied to.
        loop {
            match in_rx.recv().await {
                Some(SignalMessage::Registered { role, .. }) => {
                    info!("registered as {role:?}");
                    break;
                }
                Some(SignalMessage::Error { message }) => {
                    // Not fatal: the supervisor keeps retrying, and a PIN clash or a
                    // server still starting up resolves itself.
                    warn!("signaling rejected registration: {message}");
                }
                Some(_) => {}
                None => anyhow::bail!("signaling supervisor stopped before registering"),
            }
        }

        Ok(Self {
            outbound: out_tx,
            inbound: in_rx,
        })
    }
}

/// Own the socket. Reconnect and re-register forever; never surface a closed channel
/// to the host, because the host treats that as "shut down".
async fn supervise(
    url: String,
    registration: HostRegistration,
    mut out_rx: mpsc::UnboundedReceiver<SignalMessage>,
    in_tx: mpsc::UnboundedSender<SignalMessage>,
) {
    let mut first_attempt = true;
    loop {
        let ws = match connect_async(&url).await {
            Ok((ws, _)) => ws,
            Err(e) => {
                if first_attempt {
                    warn!("signaling connect failed ({e}) — retrying");
                    first_attempt = false;
                }
                tokio::time::sleep(RECONNECT_DELAY).await;
                continue;
            }
        };
        first_attempt = true;
        info!("signaling connected");

        let (mut sink, mut stream) = ws.split();

        // Re-assert who we are on every connection. The server keys the session on
        // this, so skipping it after a reconnect leaves the session hostless.
        if let Ok(json) = registration.message().to_json() {
            if sink.send(Message::Text(json.into())).await.is_err() {
                tokio::time::sleep(RECONNECT_DELAY).await;
                continue;
            }
        }

        // Pump until either direction fails, then loop round and reconnect.
        loop {
            tokio::select! {
                outgoing = out_rx.recv() => {
                    let Some(msg) = outgoing else {
                        return; // host is shutting down
                    };
                    match msg.to_json() {
                        Ok(json) => {
                            if sink.send(Message::Text(json.into())).await.is_err() {
                                warn!("signaling send failed — reconnecting");
                                break;
                            }
                        }
                        Err(e) => warn!("signal encode: {e}"),
                    }
                }
                incoming = stream.next() => {
                    match incoming {
                        Some(Ok(Message::Text(text))) => {
                            match SignalMessage::from_json(&text) {
                                Ok(m) => {
                                    if in_tx.send(m).is_err() {
                                        return; // host is gone
                                    }
                                }
                                Err(e) => warn!("signal decode: {e}"),
                            }
                        }
                        Some(Ok(_)) => {}
                        Some(Err(e)) => {
                            warn!("signaling read failed ({e}) — reconnecting");
                            break;
                        }
                        None => {
                            warn!("signaling closed by server — reconnecting");
                            break;
                        }
                    }
                }
            }
        }

        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}
