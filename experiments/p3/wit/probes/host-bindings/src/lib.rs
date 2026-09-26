mod message {
    wasmtime::component::bindgen!({
        path: "../../wafer-pipeline-0.2.0",
        world: "transform-message-node",
        exports: { default: async | store },
    });
}

mod stream {
    wasmtime::component::bindgen!({
        path: "../../wafer-pipeline-0.2.0",
        world: "transform-stream-node",
        exports: { default: async | store },
    });
}

mod owned_buffer {
    wasmtime::component::bindgen!({
        path: "../../ownership-probes/owned-buffer-resource",
        world: "probe",
        imports: { default: async | store },
        exports: { default: async | store },
    });
}

mod payload_stream {
    wasmtime::component::bindgen!({
        path: "../../ownership-probes/separate-payload-stream",
        world: "probe",
        exports: { default: async | store },
    });
}
