package main

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

// Required main function for TinyGo (but not used in WASM)
func main() {
	// This function is required but not called in WASM context
}
