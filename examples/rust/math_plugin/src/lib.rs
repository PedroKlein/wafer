use wasm_bindgen::prelude::*;

// Import the `console.log` function from the `console` module of `web-sys`
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}

// Define a macro for easier console logging
macro_rules! console_log {
    ($($t:tt)*) => (log(&format_args!($($t)*).to_string()))
}

// Basic arithmetic operations
#[wasm_bindgen]
pub fn add(a: i32, b: i32) -> i32 {
    console_log!("Adding {} + {}", a, b);
    a + b
}

#[wasm_bindgen]
pub fn subtract(a: i32, b: i32) -> i32 {
    console_log!("Subtracting {} - {}", a, b);
    a - b
}

#[wasm_bindgen]
pub fn multiply(a: i32, b: i32) -> i32 {
    console_log!("Multiplying {} * {}", a, b);
    a * b
}

#[wasm_bindgen]
pub fn divide(a: i32, b: i32) -> f64 {
    if b == 0 {
        console_log!("Warning: Division by zero!");
        f64::NAN
    } else {
        console_log!("Dividing {} / {}", a, b);
        a as f64 / b as f64
    }
}

// Advanced math operations
#[wasm_bindgen]
pub fn power(base: i32, exponent: u32) -> i32 {
    console_log!("Calculating {} ^ {}", base, exponent);
    base.pow(exponent)
}

#[wasm_bindgen]
pub fn factorial(n: u32) -> u64 {
    console_log!("Calculating factorial of {}", n);
    if n == 0 || n == 1 {
        1
    } else {
        (2..=n as u64).product()
    }
}

#[wasm_bindgen]
pub fn fibonacci(n: u32) -> u64 {
    console_log!("Calculating fibonacci number at position {}", n);
    match n {
        0 => 0,
        1 => 1,
        _ => {
            let mut a = 0;
            let mut b = 1;
            for _ in 2..=n {
                let temp = a + b;
                a = b;
                b = temp;
            }
            b
        }
    }
}

#[wasm_bindgen]
pub fn gcd(mut a: u32, mut b: u32) -> u32 {
    console_log!("Calculating GCD of {} and {}", a, b);
    while b != 0 {
        let temp = b;
        b = a % b;
        a = temp;
    }
    a
}

#[wasm_bindgen]
pub fn lcm(a: u32, b: u32) -> u32 {
    console_log!("Calculating LCM of {} and {}", a, b);
    a * b / gcd(a, b)
}

// String operations
#[wasm_bindgen]
pub fn greet(name: &str) -> String {
    console_log!("Greeting {}", name);
    format!("Hello, {}! This message is from Rust WASM plugin.", name)
}

#[wasm_bindgen]
pub fn reverse_string(s: &str) -> String {
    console_log!("Reversing string: {}", s);
    s.chars().rev().collect()
}

#[wasm_bindgen]
pub fn count_vowels(s: &str) -> u32 {
    console_log!("Counting vowels in: {}", s);
    s.chars()
        .filter(|c| "aeiouAEIOU".contains(*c))
        .count() as u32
}

// Array operations
#[wasm_bindgen]
pub fn sum_array(numbers: &[i32]) -> i32 {
    console_log!("Summing array of {} numbers", numbers.len());
    numbers.iter().sum()
}

#[wasm_bindgen]
pub fn find_max(numbers: &[i32]) -> i32 {
    console_log!("Finding max in array of {} numbers", numbers.len());
    *numbers.iter().max().unwrap_or(&0)
}

#[wasm_bindgen]
pub fn find_min(numbers: &[i32]) -> i32 {
    console_log!("Finding min in array of {} numbers", numbers.len());
    *numbers.iter().min().unwrap_or(&0)
}

// Utility function to get plugin info
#[wasm_bindgen]
pub fn get_plugin_info() -> String {
    console_log!("Returning plugin information");
    "Rust Math Plugin v0.1.0 - Provides various mathematical operations and utilities".to_string()
}

// When the WASM module is instantiated, this function will be called
#[wasm_bindgen(start)]
pub fn main() {
    console_log!("Rust Math Plugin initialized!");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_arithmetic() {
        assert_eq!(add(5, 3), 8);
        assert_eq!(subtract(10, 4), 6);
        assert_eq!(multiply(6, 7), 42);
        assert_eq!(divide(10, 2), 5.0);
        assert!(divide(10, 0).is_nan());
    }

    #[test]
    fn test_advanced_math() {
        assert_eq!(power(2, 3), 8);
        assert_eq!(factorial(5), 120);
        assert_eq!(fibonacci(10), 55);
        assert_eq!(gcd(48, 18), 6);
        assert_eq!(lcm(4, 6), 12);
    }

    #[test]
    fn test_string_operations() {
        assert_eq!(greet("Test"), "Hello, Test! This message is from Rust WASM plugin.");
        assert_eq!(reverse_string("hello"), "olleh");
        assert_eq!(count_vowels("hello world"), 3);
    }

    #[test]
    fn test_array_operations() {
        let numbers = [1, 2, 3, 4, 5];
        assert_eq!(sum_array(&numbers), 15);
        assert_eq!(find_max(&numbers), 5);
        assert_eq!(find_min(&numbers), 1);
    }

    #[test]
    fn test_edge_cases() {
        assert_eq!(factorial(0), 1);
        assert_eq!(factorial(1), 1);
        assert_eq!(fibonacci(0), 0);
        assert_eq!(fibonacci(1), 1);
        assert_eq!(gcd(0, 5), 5);
        assert_eq!(count_vowels(""), 0);
        assert_eq!(sum_array(&[]), 0);
    }
}
