#![cfg(test)]
//! The MQTT sink's `max_inflight` key must reach the MQTT client. A stand-in
//! broker withholds every PUBACK for a fixed window after the first publish
//! and counts the publishes that arrive meanwhile: that count is how many
//! publishes the sink keeps waiting for a PUBACK.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::net::tcp::OwnedReadHalf;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use wafer_core::orchestrator::launch_pipeline;
use wafer_types::config::Config;

const HOLD: Duration = Duration::from_millis(500);
const CONNECT: u8 = 1;
const PUBLISH: u8 = 3;
const PINGREQ: u8 = 12;

/// Reads one MQTT packet. Every packet in this test fits a one-byte
/// remaining length, which keeps the framing trivial.
async fn read_packet(stream: &mut OwnedReadHalf) -> std::io::Result<(u8, Vec<u8>)> {
    let header = stream.read_u8().await?;
    let length = stream.read_u8().await?;
    assert!(length < 0x80, "test packets use a one-byte remaining length");
    let mut body = vec![0; usize::from(length)];
    stream.read_exact(&mut body).await?;
    Ok((header, body))
}

fn publish_packet_id(header: u8, body: &[u8]) -> [u8; 2] {
    assert_eq!((header >> 1) & 0b11, 1, "the sink publishes at QoS 1");
    let topic_end = 2 + usize::from(u16::from_be_bytes([body[0], body[1]]));
    let id = &body[topic_end..];
    [id[0], id[1]]
}

/// Accepts one client and reports how many publishes arrived while the
/// first one was still unacknowledged. Afterwards it acknowledges everything
/// so the pipeline can drain and shut down.
async fn serve(listener: TcpListener, held: oneshot::Sender<usize>) {
    let (stream, _) = listener.accept().await.unwrap();
    let (mut reader, mut writer) = stream.into_split();
    let (packets_tx, mut packets) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok(packet) = read_packet(&mut reader).await {
            if packets_tx.send(packet).is_err() {
                break;
            }
        }
    });

    let (header, _) = packets.recv().await.unwrap();
    assert_eq!(header >> 4, CONNECT);
    writer.write_all(&[0x20, 0x02, 0x00, 0x00]).await.unwrap();

    let mut pending = Vec::new();
    let mut deadline = None;
    loop {
        let packet = match deadline {
            None => packets.recv().await,
            Some(deadline) => match tokio::time::timeout_at(deadline, packets.recv()).await {
                Ok(packet) => packet,
                Err(_) => break,
            },
        };
        let (header, body) = packet.expect("the sink must stay connected while publishes wait");
        if header >> 4 == PUBLISH {
            pending.push(publish_packet_id(header, &body));
            deadline.get_or_insert_with(|| Instant::now().checked_add(HOLD).unwrap());
        }
    }
    held.send(pending.len()).unwrap();

    for id in pending {
        writer.write_all(&[0x40, 0x02, id[0], id[1]]).await.unwrap();
    }
    while let Some((header, body)) = packets.recv().await {
        let reply = match header >> 4 {
            PUBLISH => {
                let id = publish_packet_id(header, &body);
                vec![0x40, 0x02, id[0], id[1]]
            }
            PINGREQ => vec![0xd0, 0x00],
            _ => continue,
        };
        if writer.write_all(&reply).await.is_err() {
            break;
        }
    }
}

fn pipeline(port: u16, max_inflight: Option<u16>) -> Config {
    let max_inflight = max_inflight.map_or_else(String::new, |n| format!("max_inflight = {n}"));
    toml::from_str(&format!(
        r#"
[nodes.src]
type = "source"
kind = "bench-source"
rate = 10000.0
total_messages = 10000000
payload_size = 16

[nodes.out]
type = "sink"
kind = "mqtt"
broker = "127.0.0.1"
port = {port}
topic = "wafer/inflight"
qos = 1
{max_inflight}

[[edges]]
from = "src"
to = "out"
"#
    ))
    .expect("inline config must parse")
}

async fn publishes_waiting_for_puback(max_inflight: Option<u16>) -> usize {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (held_tx, held_rx) = oneshot::channel();
    let broker = tokio::spawn(serve(listener, held_tx));

    let mut orchestrator = Box::pin(launch_pipeline(pipeline(port, max_inflight), None))
        .await
        .expect("pipeline must launch");
    let held = tokio::time::timeout(Duration::from_secs(10), held_rx)
        .await
        .expect("the broker must see a publish")
        .unwrap();
    orchestrator.shutdown().await.expect("pipeline shuts down");
    broker.abort();
    held
}

#[tokio::test]
async fn max_inflight_one_waits_for_each_puback() {
    assert_eq!(publishes_waiting_for_puback(Some(1)).await, 1);
}

#[tokio::test]
async fn default_window_keeps_one_hundred_publishes_in_flight() {
    assert_eq!(publishes_waiting_for_puback(None).await, 100);
}
