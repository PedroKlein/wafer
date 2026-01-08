#![no_std]

use core::panic::PanicInfo;

// Import a host function named `hello` from module namespace "env".
#[link(wasm_import_module = "env")]
extern "C" {
    fn hello();
}

// Exported entry point: host can call this, and it calls back into the host import.
#[no_mangle]
pub extern "C" fn run() {
    unsafe { hello() }
}

// A pure computation export (no imports needed).
#[no_mangle]
pub extern "C" fn add(a: i32, b: i32) -> i32 {
    a + b
}

// Data living in the module's linear memory.
static GREETING: &[u8] = b"hi from rust wasm";

// Expose a (ptr, len) pair so the host can read bytes from linear memory.
//
// In wasm32, pointers are 32-bit offsets into linear memory, so use u32.
#[no_mangle]
pub extern "C" fn greeting_ptr() -> u32 {
    GREETING.as_ptr() as u32
}

#[no_mangle]
pub extern "C" fn greeting_len() -> u32 {
    GREETING.len() as u32
}

// Minimal panic handler for no_std.
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
