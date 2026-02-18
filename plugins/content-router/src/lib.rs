wit_bindgen::generate!({
    path: "../../wit",
    world: "router-node",
});

struct ContentRouter;

fn extract_route_field(json: &str) -> Option<&str> {
    let route_key = "\"route\"";
    let start = json.find(route_key)?;
    let after_key = &json[start + route_key.len()..];
    let colon_pos = after_key.find(':')?;
    let after_colon = &after_key[colon_pos + 1..];
    let quote_start = after_colon.find('"')?;
    let value_start = &after_colon[quote_start + 1..];
    let quote_end = value_start.find('"')?;
    Some(&value_start[..quote_end])
}

impl exports::pipeline::transform::lifecycle::Guest for ContentRouter {
    fn validate(_config: exports::pipeline::transform::lifecycle::NodeConfig) -> Option<String> {
        None
    }

    fn init(_config: exports::pipeline::transform::lifecycle::NodeConfig) -> Result<(), String> {
        Ok(())
    }

    fn close() {}
}

impl exports::pipeline::transform::router::Guest for ContentRouter {
    fn output_ports() -> Vec<String> {
        vec!["port-a".to_string(), "port-b".to_string()]
    }

    fn route(
        input: pipeline::transform::types::Envelope,
    ) -> Result<
        exports::pipeline::transform::router::RouteResult,
        pipeline::transform::types::ProcessError,
    > {
        let bytes = match &input.payload {
            pipeline::transform::types::Payload::Raw(b) => b,
        };

        let text = match core::str::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => {
                return Ok(exports::pipeline::transform::router::RouteResult {
                    port: "port-a".to_string(),
                    envelope: input,
                });
            }
        };

        let port = match extract_route_field(text) {
            Some("a") => "port-a",
            Some("b") => "port-b",
            _ => "port-a",
        };

        Ok(exports::pipeline::transform::router::RouteResult {
            port: port.to_string(),
            envelope: input,
        })
    }
}

export!(ContentRouter);
