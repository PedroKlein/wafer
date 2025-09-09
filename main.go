package main

import (
	"fmt"
	"log"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/bytecodealliance/wasmtime-go/v25"
)

// Plugin represents a loaded WASM plugin
type Plugin struct {
	Name     string
	Instance *wasmtime.Instance
	Module   *wasmtime.Module
}

// PluginManager manages WASM plugins
type PluginManager struct {
	engine  *wasmtime.Engine
	store   *wasmtime.Store
	plugins map[string]*Plugin
}

// NewPluginManager creates a new plugin manager
func NewPluginManager() *PluginManager {
	engine := wasmtime.NewEngine()
	store := wasmtime.NewStore(engine)

	return &PluginManager{
		engine:  engine,
		store:   store,
		plugins: make(map[string]*Plugin),
	}
}

// LoadPlugin loads a WASM plugin from file
func (pm *PluginManager) LoadPlugin(wasmPath string) (*Plugin, error) {
	// Read the WASM file
	wasmBytes, err := os.ReadFile(wasmPath)
	if err != nil {
		return nil, fmt.Errorf("failed to read WASM file: %w", err)
	}

	// Compile the WASM module
	module, err := wasmtime.NewModule(pm.engine, wasmBytes)
	if err != nil {
		return nil, fmt.Errorf("failed to compile WASM module: %w", err)
	}

	// Create an instance of the module
	instance, err := wasmtime.NewInstance(pm.store, module, []wasmtime.AsExtern{})
	if err != nil {
		return nil, fmt.Errorf("failed to create WASM instance: %w", err)
	}

	// Extract plugin name from path
	pluginName := strings.TrimSuffix(filepath.Base(wasmPath), ".wasm")

	plugin := &Plugin{
		Name:     pluginName,
		Instance: instance,
		Module:   module,
	}

	// Store the plugin
	pm.plugins[pluginName] = plugin

	return plugin, nil
}

// LoadPluginsFromDirectory loads all WASM plugins from a directory
func (pm *PluginManager) LoadPluginsFromDirectory(dirPath string) error {
	entries, err := os.ReadDir(dirPath)
	if err != nil {
		return fmt.Errorf("failed to read directory %s: %w", dirPath, err)
	}

	for _, entry := range entries {
		if entry.IsDir() {
			continue
		}

		if strings.HasSuffix(entry.Name(), ".wasm") {
			pluginPath := filepath.Join(dirPath, entry.Name())
			plugin, err := pm.LoadPlugin(pluginPath)
			if err != nil {
				log.Printf("Warning: Failed to load plugin %s: %v", entry.Name(), err)
				continue
			}
			fmt.Printf("Successfully loaded plugin: %s\n", plugin.Name)
		}
	}

	return nil
}

// GetPlugin returns a loaded plugin by name
func (pm *PluginManager) GetPlugin(name string) (*Plugin, bool) {
	plugin, exists := pm.plugins[name]
	return plugin, exists
}

// ListPlugins returns all loaded plugin names
func (pm *PluginManager) ListPlugins() []string {
	names := make([]string, 0, len(pm.plugins))
	for name := range pm.plugins {
		names = append(names, name)
	}
	return names
}

// CallFunction calls a function from a WASM plugin
func (pm *PluginManager) CallFunction(plugin *Plugin, functionName string, args ...interface{}) (interface{}, error) {
	// Get the function from the instance
	fn := plugin.Instance.GetFunc(pm.store, functionName)
	if fn == nil {
		return nil, fmt.Errorf("function %s not found in plugin %s", functionName, plugin.Name)
	}

	// Convert args to WASM values
	wasmArgs := make([]interface{}, len(args))
	copy(wasmArgs, args)

	// Call the function
	result, err := fn.Call(pm.store, wasmArgs...)
	if err != nil {
		return nil, fmt.Errorf("failed to call function %s in plugin %s: %w", functionName, plugin.Name, err)
	}

	return result, nil
}

// GetExportedFunctions returns a list of exported functions from a plugin
func (pm *PluginManager) GetExportedFunctions(plugin *Plugin) []string {
	exports := plugin.Module.Exports()
	var functions []string

	for _, export := range exports {
		if export.Type().FuncType() != nil {
			functions = append(functions, export.Name())
		}
	}

	return functions
}

// FunctionCall represents a function call request
type FunctionCall struct {
	PluginName   string
	FunctionName string
	Args         []interface{}
}

// FunctionResult represents the result of a function call
type FunctionResult struct {
	PluginName   string
	FunctionName string
	Args         []interface{}
	Result       interface{}
	Error        error
	Duration     int64 // nanoseconds
}

// CallFunctionParallel calls multiple functions across plugins in parallel
func (pm *PluginManager) CallFunctionParallel(calls []FunctionCall) []FunctionResult {
	results := make([]FunctionResult, len(calls))
	var wg sync.WaitGroup

	for i, call := range calls {
		wg.Add(1)
		go func(index int, fc FunctionCall) {
			defer wg.Done()

			start := time.Now()
			plugin, exists := pm.GetPlugin(fc.PluginName)
			if !exists {
				results[index] = FunctionResult{
					PluginName:   fc.PluginName,
					FunctionName: fc.FunctionName,
					Args:         fc.Args,
					Error:        fmt.Errorf("plugin %s not found", fc.PluginName),
					Duration:     time.Since(start).Nanoseconds(),
				}
				return
			}

			result, err := pm.CallFunction(plugin, fc.FunctionName, fc.Args...)
			results[index] = FunctionResult{
				PluginName:   fc.PluginName,
				FunctionName: fc.FunctionName,
				Args:         fc.Args,
				Result:       result,
				Error:        err,
				Duration:     time.Since(start).Nanoseconds(),
			}
		}(i, call)
	}

	wg.Wait()
	return results
}

// LoadPluginsParallel loads multiple plugins in parallel
func (pm *PluginManager) LoadPluginsParallel(pluginPaths []string) []error {
	errors := make([]error, len(pluginPaths))
	var wg sync.WaitGroup

	for i, path := range pluginPaths {
		wg.Add(1)
		go func(index int, pluginPath string) {
			defer wg.Done()

			_, err := pm.LoadPlugin(pluginPath)
			errors[index] = err
		}(i, path)
	}

	wg.Wait()
	return errors
}

// BenchmarkFunction runs a function multiple times and returns performance stats
type BenchmarkResult struct {
	FunctionCall
	Iterations    int
	TotalDuration int64 // nanoseconds
	AvgDuration   int64 // nanoseconds
	MinDuration   int64 // nanoseconds
	MaxDuration   int64 // nanoseconds
	Errors        int
}

// BenchmarkFunction benchmarks a specific function call
func (pm *PluginManager) BenchmarkFunction(call FunctionCall, iterations int) BenchmarkResult {
	result := BenchmarkResult{
		FunctionCall: call,
		Iterations:   iterations,
		MinDuration:  int64(^uint64(0) >> 1), // max int64
	}

	plugin, exists := pm.GetPlugin(call.PluginName)
	if !exists {
		result.Errors = iterations
		return result
	}

	for i := 0; i < iterations; i++ {
		start := time.Now()
		_, err := pm.CallFunction(plugin, call.FunctionName, call.Args...)
		duration := time.Since(start).Nanoseconds()

		if err != nil {
			result.Errors++
			continue
		}

		result.TotalDuration += duration
		if duration < result.MinDuration {
			result.MinDuration = duration
		}
		if duration > result.MaxDuration {
			result.MaxDuration = duration
		}
	}

	successfulRuns := iterations - result.Errors
	if successfulRuns > 0 {
		result.AvgDuration = result.TotalDuration / int64(successfulRuns)
	}

	return result
}

// Close cleans up resources
func (pm *PluginManager) Close() {
	// The store and engine will be garbage collected
}

func main() {
	fmt.Println("WASM Plugin Loader - Wafer POC")
	fmt.Println("================================")

	// Create plugin manager
	pluginManager := NewPluginManager()
	defer pluginManager.Close()

	// Try to load plugins from the plugins directory
	pluginsDir := "plugins"
	if _, err := os.Stat(pluginsDir); err == nil {
		fmt.Printf("Loading plugins from %s directory...\n", pluginsDir)
		err := pluginManager.LoadPluginsFromDirectory(pluginsDir)
		if err != nil {
			log.Printf("Error loading plugins: %v", err)
		}
	} else {
		fmt.Printf("Plugins directory '%s' not found or empty\n", pluginsDir)
	}

	// List loaded plugins
	plugins := pluginManager.ListPlugins()
	if len(plugins) > 0 {
		fmt.Printf("\nLoaded plugins (%d):\n", len(plugins))
		for _, name := range plugins {
			plugin, _ := pluginManager.GetPlugin(name)
			functions := pluginManager.GetExportedFunctions(plugin)
			fmt.Printf("  - %s (functions: %v)\n", name, functions)
		}

		// Example: Try to call a function if available
		if len(plugins) > 0 {
			fmt.Println("\nTesting sequential function calls...")
			plugin, _ := pluginManager.GetPlugin(plugins[0])

			// Try common function names
			testFunctions := []struct {
				name string
				args []interface{}
			}{
				{"add", []interface{}{5, 3}},
				{"multiply", []interface{}{4, 7}},
				{"hello", []interface{}{}},
			}

			for _, test := range testFunctions {
				result, err := pluginManager.CallFunction(plugin, test.name, test.args...)
				if err != nil {
					fmt.Printf("  %s: not available (%v)\n", test.name, err)
				} else {
					fmt.Printf("  %s(%v) = %v\n", test.name, test.args, result)
				}
			}

			// Demonstrate parallel execution across multiple plugins
			if len(plugins) >= 1 {
				fmt.Println("\nTesting parallel function calls...")

				parallelCalls := []FunctionCall{}
				for _, pluginName := range plugins {
					plugin, exists := pluginManager.GetPlugin(pluginName)
					if !exists {
						continue
					}
					functions := pluginManager.GetExportedFunctions(plugin)

					// Add some parallel calls for each plugin
					for _, funcName := range functions {
						if funcName == "add" {
							parallelCalls = append(parallelCalls, FunctionCall{
								PluginName:   pluginName,
								FunctionName: funcName,
								Args:         []interface{}{10, 20},
							})
						}
						if funcName == "multiply" {
							parallelCalls = append(parallelCalls, FunctionCall{
								PluginName:   pluginName,
								FunctionName: funcName,
								Args:         []interface{}{6, 9},
							})
						}
					}
				}

				if len(parallelCalls) > 0 {
					start := time.Now()
					results := pluginManager.CallFunctionParallel(parallelCalls)
					duration := time.Since(start)

					fmt.Printf("  Executed %d function calls in parallel in %v\n", len(parallelCalls), duration)
					for _, result := range results {
						if result.Error != nil {
							fmt.Printf("  %s.%s(%v): ERROR - %v (took %dns)\n",
								result.PluginName, result.FunctionName, result.Args, result.Error, result.Duration)
						} else {
							fmt.Printf("  %s.%s(%v) = %v (took %dns)\n",
								result.PluginName, result.FunctionName, result.Args, result.Result, result.Duration)
						}
					}

					// Benchmark a function if available
					if len(results) > 0 && results[0].Error == nil {
						fmt.Println("\nBenchmarking function performance...")
						benchmark := pluginManager.BenchmarkFunction(FunctionCall{
							PluginName:   results[0].PluginName,
							FunctionName: results[0].FunctionName,
							Args:         results[0].Args,
						}, 1000)

						fmt.Printf("  Function: %s.%s(%v)\n", benchmark.PluginName, benchmark.FunctionName, benchmark.Args)
						fmt.Printf("  Iterations: %d (errors: %d)\n", benchmark.Iterations, benchmark.Errors)
						fmt.Printf("  Total time: %dns (%.2fms)\n", benchmark.TotalDuration, float64(benchmark.TotalDuration)/1e6)
						fmt.Printf("  Average: %dns (%.2fμs)\n", benchmark.AvgDuration, float64(benchmark.AvgDuration)/1e3)
						fmt.Printf("  Min: %dns, Max: %dns\n", benchmark.MinDuration, benchmark.MaxDuration)
					}
				}
			}
		}
	} else {
		fmt.Println("\nNo plugins loaded.")
		fmt.Println("\nTo test the plugin system:")
		fmt.Println("1. Create a WASM file (see plugins/README.md for examples)")
		fmt.Println("2. Place it in the 'plugins' directory")
		fmt.Println("3. Run this program again")
	}

	fmt.Println("\nPlugin manager ready!")
}
