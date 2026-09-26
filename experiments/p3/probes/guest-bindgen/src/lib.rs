wit_bindgen::generate!({
    path: "wit",
    world: "guest",
    async: true,
});

struct Probe;

export!(Probe);

impl exports::wafer::p3_probe::probe::Guest for Probe {
    async fn transform(payload: Vec<u8>) -> Vec<u8> {
        payload
    }
}
