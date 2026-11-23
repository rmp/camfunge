//! WebSocket control server. One thread per client; the render loop talks to clients only through
//! channels, so a slow or stuck client can never stall the video.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::{Message, Utf8Bytes};

use crate::protocol::{ClientMessage, ServerMessage};

pub type ClientId = u64;

#[derive(Debug)]
pub enum Event {
    Connected(ClientId),
    Message(ClientId, ClientMessage),
    /// A text frame that was not a valid [`ClientMessage`].
    Invalid(ClientId, String),
    Disconnected(ClientId),
}

enum Outgoing {
    Text(String),
    Binary(Vec<u8>),
}

/// Origins a browser-based client may connect from. Browsers always send `Origin` on WebSocket upgrades,
/// so this stops arbitrary web pages from driving the service or watching the camera; native clients
/// (no `Origin`) are always allowed, since they could reach the camera directly anyway.
#[derive(Debug, Clone)]
pub struct OriginPolicy {
    pub allowed: Vec<String>,
}

impl Default for OriginPolicy {
    fn default() -> Self {
        // Tauri's webview origins on macOS/Linux and Windows.
        OriginPolicy { allowed: ["tauri://localhost", "http://tauri.localhost", "https://tauri.localhost"].map(String::from).to_vec() }
    }
}

impl OriginPolicy {
    pub fn allows(&self, origin: Option<&str>) -> bool {
        match origin {
            None => true,
            Some(o) => self.allowed.iter().any(|a| a == "*" || a.eq_ignore_ascii_case(o)),
        }
    }
}

const OUTBOX: usize = 16;

pub struct Server {
    pub addr: SocketAddr,
    clients: Arc<Mutex<HashMap<ClientId, SyncSender<Outgoing>>>>,
    events: Receiver<Event>,
}

impl Server {
    pub fn bind(addr: SocketAddr, origins: OriginPolicy) -> Result<Self> {
        let listener = TcpListener::bind(addr).with_context(|| format!("could not listen on {addr}"))?;
        let addr = listener.local_addr()?;
        let clients: Arc<Mutex<HashMap<ClientId, SyncSender<Outgoing>>>> = Default::default();
        let (events_tx, events) = mpsc::channel();
        let accept_clients = clients.clone();
        std::thread::Builder::new().name("ws accept".into()).spawn(move || {
            let next_id = AtomicU64::new(1);
            for stream in listener.incoming().flatten() {
                let id = next_id.fetch_add(1, Ordering::Relaxed);
                let (clients, events, origins) = (accept_clients.clone(), events_tx.clone(), origins.clone());
                let _ = std::thread::Builder::new().name(format!("ws client {id}")).spawn(move || {
                    serve(stream, id, &origins, &clients, &events);
                    clients.lock().unwrap().remove(&id);
                    let _ = events.send(Event::Disconnected(id));
                });
            }
        })?;
        Ok(Server { addr, clients, events })
    }

    /// Everything clients have said since the last call.
    pub fn poll(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    fn push(&self, id: ClientId, out: Outgoing, droppable: bool) {
        let mut clients = self.clients.lock().unwrap();
        if let Some(tx) = clients.get(&id) {
            match tx.try_send(out) {
                Ok(()) => {}
                // Preview frames are dropped for a slow client; control messages must not be, so a client too
                // far behind to take one is disconnected (it will reconnect and get a fresh hello).
                Err(TrySendError::Full(_)) if droppable => {}
                Err(_) => {
                    clients.remove(&id);
                }
            }
        }
    }

    pub fn send(&self, id: ClientId, msg: &ServerMessage) {
        self.push(id, Outgoing::Text(serde_json::to_string(msg).expect("serialisable")), false);
    }

    pub fn broadcast(&self, msg: &ServerMessage) {
        let text = serde_json::to_string(msg).expect("serialisable");
        let ids: Vec<_> = self.clients.lock().unwrap().keys().copied().collect();
        for id in ids {
            self.push(id, Outgoing::Text(text.clone()), false);
        }
    }

    pub fn send_preview(&self, id: ClientId, jpeg: Vec<u8>) {
        self.push(id, Outgoing::Binary(jpeg), true);
    }

    pub fn client_count(&self) -> usize {
        self.clients.lock().unwrap().len()
    }
}

fn serve(stream: TcpStream, id: ClientId, origins: &OriginPolicy, clients: &Mutex<HashMap<ClientId, SyncSender<Outgoing>>>, events: &Sender<Event>) {
    let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        let origin = req.headers().get("origin").and_then(|o| o.to_str().ok());
        if origins.allows(origin) {
            Ok(resp)
        } else {
            let mut refusal = ErrorResponse::new(Some(format!("origin {} not allowed", origin.unwrap_or_default())));
            *refusal.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
            Err(refusal)
        }
    };
    let _ = stream.set_nodelay(true);
    let Ok(mut ws) = tungstenite::accept_hdr(stream, check) else { return };
    // Short read timeouts let one thread both read requests and write queued output.
    let _ = ws.get_ref().set_read_timeout(Some(Duration::from_millis(5)));
    let (tx, rx) = mpsc::sync_channel(OUTBOX);
    clients.lock().unwrap().insert(id, tx);
    if events.send(Event::Connected(id)).is_err() {
        return;
    }
    loop {
        loop {
            let msg = match rx.try_recv() {
                Ok(Outgoing::Text(t)) => Message::Text(Utf8Bytes::from(t)),
                Ok(Outgoing::Binary(b)) => Message::Binary(b.into()),
                Err(mpsc::TryRecvError::Empty) => break,
                // The render loop dropped us (too slow, or shutting down).
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = ws.close(None);
                    let _ = ws.flush();
                    return;
                }
            };
            if ws.send(msg).is_err() {
                return;
            }
        }
        match ws.read() {
            Ok(Message::Text(text)) => {
                let event = match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(m) => Event::Message(id, m),
                    Err(e) => Event::Invalid(id, e.to_string()),
                };
                if events.send(event).is_err() {
                    return;
                }
            }
            Ok(Message::Close(_)) => return,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_policy() {
        let p = OriginPolicy::default();
        assert!(p.allows(None), "native clients");
        assert!(p.allows(Some("tauri://localhost")));
        assert!(p.allows(Some("http://tauri.localhost")));
        assert!(!p.allows(Some("https://evil.example")));
        assert!(!p.allows(Some("http://localhost:8080")));
        let open = OriginPolicy { allowed: vec!["*".into()] };
        assert!(open.allows(Some("https://anything.example")));
    }
}
