//! Guest SDK for WAFER pipeline plugins.
//!
//! Provides macros and utilities for writing Wasm Component Model plugins.
//! Macros use `macro_rules!` because WIT-generated types are local to each plugin —
//! a separate crate cannot reference them as functions.

#[macro_export]
macro_rules! output_from {
    ($input:expr, $payload:expr) => {{
        let __input = &$input;
        OutputMessage {
            id: __input.id.clone(),
            timestamp: __input.timestamp,
            source: __input.source.clone(),
            content_type: __input.content_type.clone(),
            metadata: __input.metadata.clone(),
            payload: $payload,
        }
    }};
}

#[macro_export]
macro_rules! output_with_type {
    ($input:expr, $payload:expr, $content_type:expr) => {{
        let __input = &$input;
        OutputMessage {
            id: __input.id.clone(),
            timestamp: __input.timestamp,
            source: __input.source.clone(),
            content_type: $content_type.to_string(),
            metadata: __input.metadata.clone(),
            payload: $payload,
        }
    }};
}

#[macro_export]
macro_rules! payload_bytes {
    ($input:expr) => {
        $input.payload.read_all()
    };
}

#[macro_export]
macro_rules! payload_as_str {
    ($input:expr) => {{
        let bytes = $input.payload.read_all();
        String::from_utf8(bytes).map_err(|_| bad_input!("payload is not valid UTF-8"))
    }};
}

/// Construct a `ProcessError::BadInput` — input is malformed or missing fields.
#[macro_export]
macro_rules! bad_input {
    ($reason:expr) => {
        ProcessError::BadInput($reason.to_string())
    };
}

/// Construct a `ProcessError::DependencyFailed` — external resource is down.
#[macro_export]
macro_rules! dependency_failed {
    ($reason:expr) => {
        ProcessError::DependencyFailed($reason.to_string())
    };
}

/// Construct a `ProcessError::ProcessingFailed` — unexpected condition in plugin code.
#[macro_export]
macro_rules! processing_failed {
    ($reason:expr) => {
        ProcessError::ProcessingFailed($reason.to_string())
    };
}

/// Construct a `ProcessError::TimedOut` — generally produced by the host, not plugins.
#[macro_export]
macro_rules! timed_out {
    () => {
        ProcessError::TimedOut
    };
}

/// Construct a `ProcessError::Unrecoverable` — cannot recover, teardown needed.
#[macro_export]
macro_rules! unrecoverable {
    ($reason:expr) => {
        ProcessError::Unrecoverable($reason.to_string())
    };
}

/// Declare a thread-local `RefCell<Option<T>>` for plugin state.
///
/// Call once at module level. Safe because Wasm is single-threaded.
#[macro_export]
macro_rules! define_state {
    ($type:ty) => {
        thread_local! {
            static __WAFER_STATE: std::cell::RefCell<Option<$type>> =
                const { std::cell::RefCell::new(None) };
        }
    };
}

#[macro_export]
macro_rules! set_state {
    ($val:expr) => {
        __WAFER_STATE.with(|cell| *cell.borrow_mut() = Some($val))
    };
}

#[macro_export]
macro_rules! with_state {
    ($name:ident => $body:expr) => {
        __WAFER_STATE.with(|cell| {
            let mut borrow = cell.borrow_mut();
            let $name =
                borrow.as_mut().expect("plugin not initialized: init() must be called first");
            $body
        })
    };
}

#[macro_export]
macro_rules! log_info {
    ($msg:expr) => {
        wafer::pipeline::logging::log(LogLevel::Info, $msg)
    };
}

#[macro_export]
macro_rules! log_warn {
    ($msg:expr) => {
        wafer::pipeline::logging::log(LogLevel::Warn, $msg)
    };
}

#[macro_export]
macro_rules! log_error {
    ($msg:expr) => {
        wafer::pipeline::logging::log(LogLevel::Error, $msg)
    };
}

/// # Errors
///
/// Returns a human-readable error string if deserialization fails.
#[cfg(feature = "serde")]
#[must_use = "parse result should be checked"]
pub fn parse_config<T: serde::de::DeserializeOwned>(json: &str) -> Result<T, String> {
    serde_json::from_str(json).map_err(|e| format!("config parse error: {e}"))
}

#[cfg(test)]
mod tests {

    // --- parse_config tests (feature-gated) ---

    #[cfg(feature = "serde")]
    mod parse_config_tests {
        use crate::parse_config;

        #[derive(serde::Deserialize, Debug, PartialEq)]
        struct TestConfig {
            threshold: f64,
            mode: String,
        }

        #[test]
        fn test_parse_config_valid_json() {
            let json = r#"{"threshold": 42.5, "mode": "fast"}"#;
            let result: Result<TestConfig, _> = parse_config(json);
            let config = result.unwrap();
            assert!((config.threshold - 42.5).abs() < f64::EPSILON);
            assert_eq!(config.mode, "fast");
        }

        #[test]
        fn test_parse_config_invalid_json() {
            let json = "not valid json {{{";
            let result: Result<TestConfig, _> = parse_config(json);
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("config parse error"));
        }

        #[test]
        fn test_parse_config_missing_field() {
            let json = r#"{"threshold": 10.0}"#;
            let result: Result<TestConfig, _> = parse_config(json);
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.contains("mode"), "Error should mention missing field: {err}");
        }
    }

    // --- State macro tests ---

    #[test]
    fn test_define_state_and_set_state() {
        // Verify the macro expands and works at runtime
        define_state!(u64);
        set_state!(42u64);
        with_state!(state => {
            assert_eq!(*state, 42);
            *state = 100;
        });
        with_state!(state => {
            assert_eq!(*state, 100);
        });
    }

    // --- Error constructor macro tests ---
    // These test the macro syntax by defining local stand-in types that match
    // the WIT-generated ProcessError shape. In real plugins, the WIT types are used.

    #[derive(Debug, PartialEq)]
    enum ProcessError {
        BadInput(String),
        DependencyFailed(String),
        ProcessingFailed(String),
        TimedOut,
        Unrecoverable(String),
    }

    #[test]
    fn test_error_constructors() {
        let e1 = bad_input!("invalid payload");
        assert_eq!(e1, ProcessError::BadInput("invalid payload".to_string()));

        let e2 = dependency_failed!("db connection refused");
        assert_eq!(e2, ProcessError::DependencyFailed("db connection refused".to_string()));

        let e3 = processing_failed!("unexpected null");
        assert_eq!(e3, ProcessError::ProcessingFailed("unexpected null".to_string()));

        let e4 = timed_out!();
        assert_eq!(e4, ProcessError::TimedOut);

        let e5 = unrecoverable!("corrupted state");
        assert_eq!(e5, ProcessError::Unrecoverable("corrupted state".to_string()));
    }
}
