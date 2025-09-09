package main

// TinyGo WASM plugin for mathematical operations
// This creates a pure WASM module without external dependencies

// Basic arithmetic operations
//
//export add
func add(a, b int32) int32 {
	return a + b
}

//export subtract
func subtract(a, b int32) int32 {
	return a - b
}

//export multiply
func multiply(a, b int32) int32 {
	return a * b
}

//export divide
func divide(a, b int32) int32 {
	if b == 0 {
		return 0 // Safe division by zero
	}
	return a / b
}

// Advanced operations
//
//export power
func power(base, exp int32) int32 {
	if exp < 0 {
		return 0
	}
	if exp == 0 {
		return 1
	}
	result := int32(1)
	for i := int32(0); i < exp; i++ {
		result *= base
	}
	return result
}

//export factorial
func factorial(n int32) int32 {
	if n < 0 {
		return 0
	}
	if n <= 1 {
		return 1
	}
	result := int32(1)
	for i := int32(2); i <= n; i++ {
		result *= i
	}
	return result
}

//export fibonacci
func fibonacci(n int32) int32 {
	if n < 0 {
		return 0
	}
	if n <= 1 {
		return n
	}
	a, b := int32(0), int32(1)
	for i := int32(2); i <= n; i++ {
		a, b = b, a+b
	}
	return b
}

//export gcd
func gcd(a, b int32) int32 {
	if a < 0 {
		a = -a
	}
	if b < 0 {
		b = -b
	}
	for b != 0 {
		a, b = b, a%b
	}
	return a
}

//export lcm
func lcm(a, b int32) int32 {
	if a == 0 || b == 0 {
		return 0
	}
	gcdVal := gcd(a, b)
	if a < 0 {
		a = -a
	}
	if b < 0 {
		b = -b
	}
	return (a * b) / gcdVal
}

// Utility functions
//
//export abs_value
func abs_value(n int32) int32 {
	if n < 0 {
		return -n
	}
	return n
}

//export max
func max(a, b int32) int32 {
	if a > b {
		return a
	}
	return b
}

//export min
func min(a, b int32) int32 {
	if a < b {
		return a
	}
	return b
}

//export is_prime
func is_prime(n int32) int32 {
	if n < 2 {
		return 0 // false
	}
	if n == 2 {
		return 1 // true
	}
	if n%2 == 0 {
		return 0 // false
	}
	for i := int32(3); i*i <= n; i += 2 {
		if n%i == 0 {
			return 0 // false
		}
	}
	return 1 // true
}

//export square_root
func square_root(n int32) int32 {
	if n < 0 {
		return 0
	}
	if n == 0 || n == 1 {
		return n
	}

	// Simple integer square root using binary search
	start, end := int32(1), n
	for start <= end {
		mid := (start + end) / 2
		square := mid * mid

		if square == n {
			return mid
		}
		if square < n {
			start = mid + 1
		} else {
			end = mid - 1
		}
	}
	return end
}

// Plugin information
//
//export get_plugin_version
func get_plugin_version() int32 {
	return 100 // Version 1.0.0
}

// Required main function for TinyGo (but not used in WASM)
func main() {
	// This function is required but not called in WASM context
}
