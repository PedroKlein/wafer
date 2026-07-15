wit_bindgen::generate!({
    path: "../../wit/node",
    world: "transform-node",
    generate_all,
});

struct Plugin;

impl exports::pipeline::node::lifecycle::Guest for Plugin {
    fn validate(_config: exports::pipeline::node::lifecycle::NodeConfig) -> Option<String> { None }
    fn init(_config: exports::pipeline::node::lifecycle::NodeConfig) -> Result<(), exports::pipeline::node::lifecycle::ProcessError> { Ok(()) }
    fn close() {}
}

impl exports::pipeline::node::transform::Guest for Plugin {
    fn process(_input: exports::pipeline::node::transform::Message) -> Result<exports::pipeline::node::transform::OutputMessage, exports::pipeline::node::transform::ProcessError> {
        unimplemented!()
    }
}

export!(Plugin);
