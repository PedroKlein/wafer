#[cfg(feature = "message")]
wit_bindgen::generate!({
    path: "../../wafer-pipeline-0.2.0",
    world: "transform-message-node",
    stubs,
});

#[cfg(feature = "stream")]
wit_bindgen::generate!({
    path: "../../wafer-pipeline-0.2.0",
    world: "transform-stream-node",
    stubs,
});

#[cfg(feature = "owned-buffer")]
wit_bindgen::generate!({
    path: "../../ownership-probes/owned-buffer-resource",
    world: "probe",
    stubs,
});

#[cfg(feature = "payload-stream")]
wit_bindgen::generate!({
    path: "../../ownership-probes/separate-payload-stream",
    world: "probe",
    stubs,
});
