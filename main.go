package main

import (
	"fmt"
	"log"
	"os"
	"path/filepath"
	"strings"

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
	linker  *wasmtime.Linker
	plugins map[string]*Plugin
}

// NewPluginManager creates a new plugin manager
func NewPluginManager() *PluginManager {
	engine := wasmtime.NewEngine()
	store := wasmtime.NewStore(engine)

	wasi := wasmtime.NewWasiConfig()
	wasi.InheritStdout()
	wasi.InheritStderr()
	wasi.InheritStdin()
	store.SetWasi(wasi)

	linker := wasmtime.NewLinker(engine)
	if err := linker.DefineWasi(); err != nil {
		log.Fatal(err)
	}

	return &PluginManager{
		engine:  engine,
		store:   store,
		linker:  linker,
		plugins: make(map[string]*Plugin),
	}
}

// LoadPlugin loads a WASM plugin from file
func (pm *PluginManager) LoadPlugin(wasmPath string) (*Plugin, error) {
	module, err := wasmtime.NewModuleFromFile(pm.engine, wasmPath)
	if err != nil {
		return nil, fmt.Errorf("failed to compile WASM module: %w", err)
	}

	// Create an instance of the module
	instance, err := pm.linker.Instantiate(pm.store, module)
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
func (pm *PluginManager) CallFunction(plugin *Plugin, functionName string, args ...any) (any, error) {
	// Get the function from the instance
	fn := plugin.Instance.GetFunc(pm.store, functionName)
	if fn == nil {
		return nil, fmt.Errorf("function %s not found in plugin %s", functionName, plugin.Name)
	}

	// Call the function
	result, err := fn.Call(pm.store, args...)
	if err != nil {
		return nil, fmt.Errorf("failed to call function %s in plugin %s: %w", functionName, plugin.Name, err)
	}

	return result, nil
}

func (p *Plugin) ImportsNeeded() []string {
	var imports []string
	for _, im := range p.Module.Imports() {
		imports = append(imports, fmt.Sprintf("%q::%q", im.Module(), *im.Name()))
	}
	return imports
}

func (p *Plugin) ExportsAvailable() []string {
	var exports []string
	for _, ex := range p.Module.Exports() {
		exports = append(exports, fmt.Sprintf("%q", ex.Name()))
	}
	return exports
}

func main() {
	fmt.Println("WASM Plugin Loader - Wafer POC")
	fmt.Println("================================")

	// Create plugin manager
	pluginManager := NewPluginManager()

	// Try to load plugins from the plugins directory
	pluginsDir := "plugins"
	if _, err := os.Stat(pluginsDir); err != nil {
		log.Fatalf("Plugins directory '%s' not found or empty\n", pluginsDir)
	}

	fmt.Printf("Loading plugins from %s directory...\n", pluginsDir)
	err := pluginManager.LoadPluginsFromDirectory(pluginsDir)
	if err != nil {
		log.Printf("Error loading plugins: %v", err)
	}

	// List loaded plugins
	plugins := pluginManager.ListPlugins()
	if len(plugins) == 0 {
		log.Fatal("No plugins loaded. Please add WASM files to the plugins directory.")
	}

	for _, name := range plugins {
		fmt.Printf("\nAvailable functions for plugin %q:\n", name)
		plugin, _ := pluginManager.GetPlugin(name)
		fmt.Printf("\t %s (functions: %v)\n", name, plugin.ExportsAvailable())
		fmt.Println("Testing basic function calls:")

		// Try common function names
		testFunctions := []struct {
			name string
			args []any
		}{
			{"add", []any{5, 3}},
			{"multiply", []any{4, 7}},
			{"hello", []any{}},
		}

		for _, test := range testFunctions {
			result, err := pluginManager.CallFunction(plugin, test.name, test.args...)
			if err != nil {
				fmt.Printf("\t%s: not available (%v)\n", test.name, err)
			} else {
				fmt.Printf("\t%s(%v) = %v\n", test.name, test.args, result)
			}
		}
	}
}
