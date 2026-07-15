wit_bindgen::generate!({
    path: "../../wit/node",
    world: "filter-node",
    generate_all,
});

struct Plugin;

impl exports::pipeline::node::lifecycle::Guest for Plugin {
    fn validate(_config: exports::pipeline::node::lifecycle::NodeConfig) -> Option<String> { None }
    fn init(_config: exports::pipeline::node::lifecycle::NodeConfig) -> Result<(), exports::pipeline::node::lifecycle::ProcessError> { Ok(()) }
    fn close() {}
}

impl exports::pipeline::node::filter::Guest for Plugin {
    fn evaluate(_input: exports::pipeline::node::filter::Message) -> Result<bool, exports::pipeline::node::filter::ProcessError> {
        unimplemented!()
    }
}

export!(Plugin);
