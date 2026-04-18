# WIT & Component Model

This chapter covers WIT (WebAssembly Interface Types) syntax, interface design best practices, resource types, world composition, and common anti-patterns.

## 4.1 WIT Naming Conventions

WIT uses strict naming rules:

- **All identifiers**: `kebab-case` (words separated by single hyphens)
- **Package names**: `namespace:name` or `namespace:name@semver`
- Words must be all-lowercase or all-UPPERCASE per segment
- Use `%` prefix for WIT keywords used as identifiers: `%interface`, `%world`
- Double hyphens (`--`) are not allowed

```wit
// GOOD
package my-org:data-pipeline@1.0.0;

interface message-types {
    record sensor-reading { ... }
    process-message: func(msg: sensor-reading) -> result<sensor-reading, process-error>;
}

// BAD
package myOrg:dataPipeline;   // camelCase not allowed
interface MessageTypes { ... } // PascalCase not allowed
```

## 4.2 Interface Design: Single Responsibility

Design interfaces to be small, focused, and composable. Each interface should describe a cohesive unit of functionality.

```wit
// GOOD: Small, focused interfaces
interface wall-clock {
    record datetime {
        seconds: u64,
        nanoseconds: u32,
    }
    now: func() -> datetime;
}

interface monotonic-clock {
    type instant = u64;
    now: func() -> instant;
    resolution: func() -> instant;
}

// BAD: Kitchen-sink interface
interface time-stuff {
    // wall clock, monotonic clock, timers, formatters all mixed
    now: func() -> u64;
    wall-now: func() -> datetime;
    format-time: func(t: u64) -> string;
    set-timer: func(duration: u64);
}
```

## 4.3 Separate Types into Dedicated Interfaces

The WASI ecosystem consistently uses a `types` interface for shared type definitions:

```wit
package my-org:http@0.2.0;

// Shared types in a dedicated interface
interface types {
    record request {
        method: method,
        uri: string,
        headers: list<tuple<string, string>>,
        body: list<u8>,
    }

    record response {
        status: u16,
        headers: list<tuple<string, string>>,
        body: list<u8>,
    }

    enum method {
        get,
        post,
        put,
        delete,
    }

    enum error-code {
        network-error,
        timeout,
        invalid-url,
    }
}

// Import types via `use`
interface incoming-handler {
    use types.{request, response, error-code};
    handle: func(request: request) -> result<response, error-code>;
}

interface outgoing-handler {
    use types.{request, response, error-code};
    send: func(request: request) -> result<response, error-code>;
}
```

## 4.4 Built-in Types

### Primitives

| Type | Description |
|---|---|
| `bool` | Boolean |
| `s8`, `s16`, `s32`, `s64` | Signed integers |
| `u8`, `u16`, `u32`, `u64` | Unsigned integers |
| `f32`, `f64` | Floating-point (single NaN) |
| `char` | Unicode scalar value |
| `string` | Unicode string |

### Compound Types

```wit
// Lists
list<u8>              // Byte buffer (like Vec<u8>)
list<customer>        // List of records

// Options
option<customer>      // May or may not contain a value (like Rust Option)

// Results -- use for ALL fallible operations
result<response, error-code>   // Success or error
result<u32>                    // Success with data, error with no data
result<_, u32>                 // No success data, error with data
result                         // Neither case has data

// Tuples
tuple<u64, string>             // Fixed-length heterogeneous sequence
```

### Records (structs)

```wit
record customer {
    id: u64,
    name: string,
    picture: option<list<u8>>,
    account-manager: employee,
}
```

### Variants (enums with data)

```wit
variant allowed-destinations {
    none,
    any,
    restricted(list<address>),
}
```

### Enums (no associated data)

```wit
enum color {
    hot-pink,
    lime-green,
    navy-blue,
}
```

### Flags (bitfields)

```wit
// Efficient bitfield representation
flags permissions {
    read,
    write,
    execute,
}

// Less efficient alternative (avoid):
// record permissions { read: bool, write: bool, execute: bool }
```

### Type Aliases

```wit
type buffer = list<u8>;
type http-result = result<response, error-code>;
```

## 4.5 Resources

Resources represent handles to stateful entities that exist on one side of the boundary. They can't be copied -- they must be passed by handle.

```wit
resource connection {
    // Constructor: returns an owned handle
    constructor(url: string);

    // Methods: implicit borrow<self> parameter
    execute: func(query: string) -> result<row-set, db-error>;
    close: func();

    // Static functions: no implicit self
    pool-size: static func() -> u32;
}
```

### Ownership Model

- **Owned handle**: Caller is responsible for dropping the resource
- **Borrowed handle** (`borrow<T>`): Temporary loan for the duration of a call

```wit
interface document-store {
    resource document {
        constructor(content: string);
        get-content: func() -> string;      // borrows self
        set-content: func(content: string);  // borrows self
    }

    // Takes owned document (consumes it)
    archive: func(doc: document);

    // Borrows document (returns it after)
    validate: func(doc: borrow<document>) -> bool;
}
```

### Resource Implementation (Guest Side)

Guest resources need `RefCell` because generated trait methods take `&self`:

```rust
use std::cell::RefCell;

struct MyDocument {
    content: RefCell<String>,
}

impl GuestDocument for MyDocument {
    fn new(content: String) -> Self {
        MyDocument { content: RefCell::new(content) }
    }

    fn get_content(&self) -> String {
        self.content.borrow().clone()
    }

    fn set_content(&self, content: String) {
        *self.content.borrow_mut() = content;
    }
}
```

### Resource Implementation (Host Side)

Host resources use `ResourceTable`:

```rust
impl HostDocument for MyHost {
    fn new(&mut self, content: String) -> wasmtime::Result<Resource<Document>> {
        Ok(self.table.push(Document { content })?)
    }

    fn get_content(&mut self, self_: Resource<Document>) -> wasmtime::Result<String> {
        Ok(self.table.get(&self_)?.content.clone())
    }

    fn drop(&mut self, rep: Resource<Document>) -> wasmtime::Result<()> {
        self.table.delete(rep)?;
        Ok(())
    }
}
```

## 4.6 Worlds

Worlds describe the complete contract of a component -- what it imports and exports:

```wit
world multi-function-device {
    // Import: component needs this provided by host
    import error-reporter;

    // Export interface: component provides this
    export printer;

    // Export function: component provides this
    export scan: func() -> list<u8>;
}
```

### Extending Worlds with `include`

```wit
world enhanced-proxy {
    include wasi:http/proxy;              // Inherit all imports/exports
    export my-org:telemetry/metrics;       // Add custom exports
}
```

### Inline Interfaces

For small, one-off interfaces:

```wit
world toy {
    export example: interface {
        do-nothing: func();
    }
}
```

## 4.7 Component Composition

Components can be composed -- linking one component's exports to another's imports.

### Simple Composition with `wac plug`

```bash
# Plug a dependency's exports into a consumer's imports
wac plug consumer.wasm --plug dependency.wasm -o composed.wasm
```

### Complex Composition with WAC Language

```
package my:composition;

let auth = new my:auth-impl {};
let db = new my:db-impl {};
let app = new my:app-impl {
    auth: auth.auth,
    database: db.store,
};
export app...;
```

### Composition Requirements

1. **Interface-level imports**: Bare function imports break composition
2. **Matching versions**: Import/export versions must match exactly
3. **Type compatibility**: Shared types must come from the same WIT package

## 4.8 Documentation

WIT supports doc comments:

```wit
/// Provides access to environment variables.
/// Values are not guaranteed to reflect the host's actual environment.
interface environment {
    /// Returns all environment variables as key-value pairs.
    ///
    /// The returned list may be empty if no variables are configured.
    get-environment: func() -> list<tuple<string, string>>;

    /// Returns the value of a specific variable, if set.
    get-variable: func(key: string) -> option<string>;
}
```

## 4.9 Anti-Patterns

### Bare Function Imports (breaks composition)

```wit
// BAD: Direct function imports can't be composed
world my-world {
    import do-something: func() -> string;
}

// GOOD: Wrap in an interface
interface helpers {
    do-something: func() -> string;
}

world my-world {
    import helpers;
}
```

### Missing Version Numbers

```wit
// BAD: No version -- causes composition failures
package my-org:my-service;

// GOOD: Always include semver
package my-org:my-service@1.0.0;
```

### Kitchen-Sink Interfaces

```wit
// BAD: Does too many things
interface everything {
    read-file: func(path: string) -> list<u8>;
    send-http: func(url: string) -> string;
    log: func(msg: string);
    get-time: func() -> u64;
}

// GOOD: Separate concerns
interface file-reader { read: func(path: string) -> result<list<u8>, io-error>; }
interface http-client { send: func(url: string) -> result<string, http-error>; }
interface logger { log: func(level: log-level, msg: string); }
```

### Not Using `result<T, E>` for Fallible Operations

```wit
// BAD: No error handling
lookup: func(key: string) -> string;  // What if key not found?

// GOOD: Explicit error handling
lookup: func(key: string) -> result<string, store-error>;
```

### Forgetting `RefCell` in Guest Resources

Guest resource methods receive `&self`, not `&mut self`. Without `RefCell`, you can't mutate state:

```rust
// BAD: Won't compile -- can't mutate through &self
struct Counter { count: u32 }
impl GuestCounter for Counter {
    fn increment(&self) { self.count += 1; } // ERROR
}

// GOOD: Interior mutability
struct Counter { count: RefCell<u32> }
impl GuestCounter for Counter {
    fn increment(&self) { *self.count.borrow_mut() += 1; }
}
```

### Over-Permissioned Worlds

```wit
// BAD: Plugin gets full CLI access when it only needs logging
world my-plugin {
    include wasi:cli/command;
    export transform;
}

// GOOD: Minimal permissions
world my-plugin {
    import wasi:logging/logging;
    export transform;
}
```

## References

- [WIT specification](https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md)
- [Component Model concepts](https://component-model.bytecodealliance.org/design/component-model-concepts.html)
- [Component Model book](https://component-model.bytecodealliance.org/)
- [WAC composition](https://github.com/bytecodealliance/wac)
