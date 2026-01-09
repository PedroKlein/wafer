mod component;
mod module;

use crate::component::component_host;
use crate::module::normal_host;

use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, QoS};
use std::time::Duration;

async fn run_mqtt() -> anyhow::Result<()> {
    let mut opts = MqttOptions::new("rust-client-1", "localhost", 1883);
    opts.set_keep_alive(Duration::from_secs(5));

    let (client, mut eventloop) = AsyncClient::new(opts, 10);
    client.subscribe("test/topic", QoS::AtLeastOnce).await?;

    println!("Subscribed to test/topic, waiting for messages...");

    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Incoming::Publish(p))) => {
                match std::str::from_utf8(&p.payload) {
                    Ok(s) => println!("Message on {}: {}", p.topic, s),
                    Err(_) => println!("Message on {} ({} bytes): {:?}", p.topic, p.payload.len(), p.payload),
                }
            }
            Ok(Event::Incoming(other)) => {
                println!("Incoming: {:?}", other);
            }
            Ok(Event::Outgoing(outgoing)) => {
                // Suppress noisy outgoing events; uncomment to debug
                println!("Outgoing: {:?}", outgoing);
            }
            Err(e) => {
                eprintln!("MQTT error: {:?}", e);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    // If invoked with `cargo run -- mqtt`, run the MQTT client.
    let mut args = std::env::args();
    let _bin = args.next();
    if let Some(mode) = args.next() {
        if mode == "mqtt" {
            let rt = tokio::runtime::Runtime::new()?;
            return rt.block_on(run_mqtt());
        }
    }

    // Default behavior: existing hosts
    component_host()?;
    normal_host()?;
    Ok(())
}
