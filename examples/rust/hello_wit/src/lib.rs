mod bindings {
    wit_bindgen::generate!({
        path: "./wit",
        world: "plugin",
    });

    use super::Plugin;
    export!(Plugin);
}

struct Plugin;

impl bindings::Guest for Plugin {
    fn run() -> u32 {
        // Call the host import (typed string, no pointers).
        let n = bindings::hello("called from guest");
        println!("host returned count = {n}");
        n
    }

    fn greeting() -> String {
        "hello from the component".to_string()
    }
}
