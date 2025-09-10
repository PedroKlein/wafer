package main

//export add
func Add(a, b int32) int32 {
	return a + b
}

//export subtract
func Subtract(a, b int32) int32 {
	return a - b
}

//export multiply
func Multiply(a, b int32) int32 {
	return a * b
}

//export hello
func Hello() string {
	return "Hello from Go WASM plugin!"
}

// Required main function for TinyGo (but not used in WASM)
func main() {
	// This function is required but not called in WASM context
}
