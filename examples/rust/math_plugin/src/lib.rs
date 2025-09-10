// Basic arithmetic operations
#[no_mangle]
pub extern "C" fn add(a: i32, b: i32) -> i32 {
    return a + b
}

#[no_mangle]
pub extern "C" fn hello() -> *const u8 {
    // Static string to ensure it lives long enough
    static HELLO: &str = "Hello from Rust WASM plugin!";
    HELLO.as_ptr()
}