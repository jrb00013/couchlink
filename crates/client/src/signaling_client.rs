use anyhow::{bail, Context, Result};
use couchlink_proto::SignalMessage;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{info, warn};

pub struct SignalingClient {
    pub outbound: mpsc::UnboundedSender<SignalMessage>,
    pub inbound: mpsc::UnboundedReceiver<SignalMessage>,
}

impl SignalingClient {
    pub async fn connect(url: &str) -> Result<Self> {
        let (ws, _) = connect_async(url)
            .await
            .with_context(|| format!("connect {url}"))?;
        let (mut sink, mut stream) = ws.split();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<SignalMessage>();
        let (in_tx, in_rx) = mpsc::unbounded_channel::<SignalMessage>();

        tokio::spawn(async move {
            while let Some(msg) = out_rx.recv().await {
                if let Ok(j) = msg.to_json() {
                    if sink.send(Message::Text(j.into())).await.is_err() {
                        break;
                    }
                }
            }
        });
        tokio::spawn(async move {
            while let Some(Ok(msg)) = stream.next().await {
                let Message::Text(t) = msg else { continue };
                match SignalMessage::from_json(&t) {
                    Ok(m) => {
                        if in_tx.send(m).is_err() {
                            break;
                        }
                    }
                    Err(e) => warn!("decode: {e}"),
                }
            }
        });
        Ok(Self {
            outbound: out_tx,
            inbound: in_rx,
        })
    }

    pub async fn register_player(&mut self, session_id: String, pin: String) -> Result<()> {
        self.outbound.send(SignalMessage::RegisterPlayer {
            session_id,
            pin,
            player_name: None,
        })?;
        while let Some(msg) = self.inbound.recv().await {
            match msg {
                SignalMessage::Registered { .. } => {
                    info!("player registered");
                    return Ok(());
                }
                SignalMessage::Error { message } => bail!("{message}"),
                _ => {}
            }
        }
        bail!("closed")
    }
}
