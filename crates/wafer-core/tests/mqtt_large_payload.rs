#![cfg(test)]
//! The runtime's MQTT clients must carry payloads larger than rumqttc's
//! 10 KiB default packet limit. A stand-in broker sends the source a large
//! PUBLISH and checks that a large publish from the sink reaches it.

use std::time::Duration;

use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::net::tcp::OwnedReadHalf;
use tokio::sync::oneshot;
use wafer_core::node::{Lifecycle, MqttSink, MqttSource, Sink, Source};
use wafer_core::queue::RuntimeEnvelope;

const TOPIC: &str = "wafer/large";
const PAYLOAD_BYTES: usize = 64 * 1024;
const CONNECT: u8 = 1;
const PUBLISH: u8 = 3;
const SUBSCRIBE: u8 = 8;
const PINGREQ: u8 = 12;

async fn read_packet(stream: &mut OwnedReadHalf) -> std::io::Result<(u8, Vec<u8>)> {
    let header = stream.read_u8().await?;
    let mut length = 0_usize;
    for shift in [0, 7, 14, 21] {
        let byte = stream.read_u8().await?;
        length |= usize::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            break;
        }
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await?;
    Ok((header, body))
}

fn packet(header: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![header];
    let mut length = body.len();
    loop {
        let mut byte = u8::try_from(length & 0x7f).unwrap();
        length >>= 7;
        if length > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if length == 0 {
            break;
        }
    }
    out.extend_from_slice(body);
    out
}

fn publish_body(payload: &[u8]) -> Vec<u8> {
    let topic_len = u16::try_from(TOPIC.len()).unwrap();
    let mut body = topic_len.to_be_bytes().to_vec();
    body.extend_from_slice(TOPIC.as_bytes());
    body.extend_from_slice(payload);
    body
}

fn large_payload() -> Vec<u8> {
    (0..PAYLOAD_BYTES).map(|i| u8::try_from(i % 251).unwrap()).collect()
}

async fn accept(listener: &TcpListener) -> (OwnedReadHalf, tokio::net::tcp::OwnedWriteHalf) {
    let (stream, _) = listener.accept().await.unwrap();
    let (mut reader, mut writer) = stream.into_split();
    let (header, _) = read_packet(&mut reader).await.unwrap();
    assert_eq!(header >> 4, CONNECT);
    writer.write_all(&[0x20, 0x02, 0x00, 0x00]).await.unwrap();
    (reader, writer)
}

#[tokio::test]
async fn mqtt_source_receives_payload_above_ten_kib() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let broker = tokio::spawn(async move {
        let (mut reader, mut writer) = accept(&listener).await;
        let (header, body) = read_packet(&mut reader).await.unwrap();
        assert_eq!(header >> 4, SUBSCRIBE);
        writer.write_all(&[0x90, 0x03, body[0], body[1], 0x00]).await.unwrap();
        writer.write_all(&packet(0x30, &publish_body(&large_payload()))).await.unwrap();
        while let Ok((header, _)) = read_packet(&mut reader).await {
            if header >> 4 == PINGREQ && writer.write_all(&[0xd0, 0x00]).await.is_err() {
                break;
            }
        }
    });

    let mut source = MqttSource::new("mqtt-src", "127.0.0.1", port, TOPIC, 0, "wafer-large-src");
    source.init().await.unwrap();
    let envelope = tokio::time::timeout(Duration::from_secs(10), source.poll())
        .await
        .expect("the source must deliver the large publish")
        .unwrap()
        .expect("MQTT source ended");
    assert_eq!(envelope.payload.as_ref(), large_payload().as_slice());

    source.close().await.unwrap();
    broker.abort();
}

#[tokio::test]
async fn mqtt_sink_publishes_payload_above_ten_kib() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (received_tx, received_rx) = oneshot::channel();
    let broker = tokio::spawn(async move {
        let (mut reader, mut writer) = accept(&listener).await;
        while let Ok((header, body)) = read_packet(&mut reader).await {
            match header >> 4 {
                PUBLISH => {
                    let topic_end = 2 + usize::from(u16::from_be_bytes([body[0], body[1]]));
                    received_tx.send(body[topic_end..].to_vec()).unwrap();
                    break;
                }
                PINGREQ => writer.write_all(&[0xd0, 0x00]).await.unwrap(),
                _ => {}
            }
        }
    });

    let mut sink = MqttSink::new("mqtt-out", "127.0.0.1", port, TOPIC, 0, "wafer-large-out");
    sink.init().await.unwrap();
    sink.collect(RuntimeEnvelope::new("src", Bytes::from(large_payload()))).await.unwrap();
    let received = tokio::time::timeout(Duration::from_secs(10), received_rx)
        .await
        .expect("the broker must receive the large publish")
        .unwrap();
    assert_eq!(received, large_payload());

    sink.close().await.unwrap();
    broker.abort();
}
