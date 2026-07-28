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
            .with_context(|| format!("connect signaling {url}"))?;
        let (mut sink, mut stream) = ws.split();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<SignalMessage>();
        let (in_tx, in_rx) = mpsc::unbounded_channel::<SignalMessage>();

        tokio::spawn(async move {
            while let Some(msg) = out_rx.recv().await {
                match msg.to_json() {
                    Ok(j) => {
                        if sink.send(Message::Text(j.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => warn!("signal encode: {e}"),
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
                    Err(e) => warn!("signal decode: {e}"),
                }
            }
        });

        info!("signaling connected");
        Ok(Self {
            outbound: out_tx,
            inbound: in_rx,
        })
    }

    pub async fn register_host(
        &mut self,
        session_id: String,
        pin: String,
        device_name: String,
        preset: String,
        emulator: String,
    ) -> Result<()> {
        self.outbound.send(SignalMessage::RegisterHost {
            session_id: session_id.clone(),
            pin,
            device_name: Some(device_name),
            preset: Some(preset),
            emulator: Some(emulator),
        })?;
        while let Some(msg) = self.inbound.recv().await {
            match msg {
                SignalMessage::Registered { role, .. } => {
                    info!("registered as {role:?}");
                    return Ok(());
                }
                SignalMessage::Error { message } => bail!("signaling: {message}"),
                _ => {}
            }
        }
        bail!("signaling closed before register ack")
    }
}
