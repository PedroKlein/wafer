//! A minimal in-process MQTT 3.1.1 broker for publisher tests: it answers
//! CONNECT, PUBLISH (`QoS` 1), SUBSCRIBE and PINGREQ, and lets a test withhold
//! acknowledgements, refuse sessions or drop every open connection.

#![allow(dead_code, reason = "each integration test uses the subset of controls it needs")]
#![expect(clippy::unwrap_used, reason = "test scaffolding: a failure here is a test failure")]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use bytes::BytesMut;
use rumqttc::{ConnAck, ConnectReturnCode, Packet, PubAck, QoS, SubAck, SubscribeReasonCode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

const MAX_PACKET_BYTES: usize = 1024 * 1024;

#[derive(Default)]
struct Shared {
    hold_acks: AtomicBool,
    refuse: AtomicBool,
    connections: AtomicU64,
    publishes: AtomicU64,
    generation: std::sync::Mutex<CancellationToken>,
}

pub struct FakeBroker {
    addr: SocketAddr,
    shared: Arc<Shared>,
    accept_task: tokio::task::JoinHandle<()>,
}

impl FakeBroker {
    pub async fn start() -> Self {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let shared = Arc::new(Shared::default());
        let accept_shared = Arc::clone(&shared);
        let accept_task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                accept_shared.connections.fetch_add(1, Ordering::AcqRel);
                let session = accept_shared.generation.lock().unwrap().clone();
                let shared = Arc::clone(&accept_shared);
                tokio::spawn(async move {
                    tokio::select! {
                        () = session.cancelled() => {}
                        _ = serve(stream, shared) => {}
                    }
                });
            }
        });
        Self { addr, shared, accept_task }
    }

    pub fn host(&self) -> String {
        self.addr.ip().to_string()
    }

    pub const fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Stop answering PUBLISH packets (or resume, with `false`).
    pub fn hold_acks(&self, hold: bool) {
        self.shared.hold_acks.store(hold, Ordering::Release);
    }

    /// Answer every CONNECT with a refusal.
    pub fn refuse_connections(&self) {
        self.shared.refuse.store(true, Ordering::Release);
    }

    /// Close every open connection, as a broker restart would.
    pub fn drop_connections(&self) {
        let mut generation = self.shared.generation.lock().unwrap();
        generation.cancel();
        *generation = CancellationToken::new();
    }

    pub fn connections(&self) -> u64 {
        self.shared.connections.load(Ordering::Acquire)
    }

    pub fn publishes(&self) -> u64 {
        self.shared.publishes.load(Ordering::Acquire)
    }
}

impl Drop for FakeBroker {
    fn drop(&mut self) {
        self.accept_task.abort();
        self.shared.generation.lock().unwrap().cancel();
    }
}

async fn serve(mut stream: TcpStream, shared: Arc<Shared>) -> std::io::Result<()> {
    let mut incoming = BytesMut::with_capacity(4096);
    let mut outgoing = BytesMut::with_capacity(64);
    loop {
        let packet = match Packet::read(&mut incoming, MAX_PACKET_BYTES) {
            Ok(packet) => packet,
            Err(rumqttc::Error::InsufficientBytes(_)) => {
                if stream.read_buf(&mut incoming).await? == 0 {
                    return Ok(());
                }
                continue;
            }
            Err(_) => return Ok(()),
        };
        let reply = match packet {
            Packet::Connect(_) => {
                let code = if shared.refuse.load(Ordering::Acquire) {
                    ConnectReturnCode::BadClientId
                } else {
                    ConnectReturnCode::Success
                };
                Some(Packet::ConnAck(ConnAck::new(code, false)))
            }
            Packet::Publish(publish) => {
                shared.publishes.fetch_add(1, Ordering::AcqRel);
                (publish.qos == QoS::AtLeastOnce && !shared.hold_acks.load(Ordering::Acquire))
                    .then(|| Packet::PubAck(PubAck::new(publish.pkid)))
            }
            Packet::Subscribe(subscribe) => {
                let codes = subscribe
                    .filters
                    .iter()
                    .map(|filter| SubscribeReasonCode::Success(filter.qos))
                    .collect();
                Some(Packet::SubAck(SubAck::new(subscribe.pkid, codes)))
            }
            Packet::PingReq => Some(Packet::PingResp),
            Packet::Disconnect => return Ok(()),
            _ => None,
        };
        if let Some(reply) = reply {
            outgoing.clear();
            reply.write(&mut outgoing, MAX_PACKET_BYTES).unwrap();
            stream.write_all(&outgoing).await?;
        }
    }
}
