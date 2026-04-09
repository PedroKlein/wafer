//! Error handling with standardized exit codes for waferctl.
//!
//! Exit codes follow a consistent convention:
//! - 0: Success
//! - 1: User error (bad arguments, invalid config)
//! - 2: API error (server returned error, operation failed)
//! - 3: Connection error (cannot reach server)

use std::fmt;

/// Exit codes for waferctl.
pub mod exit_code {
    /// Operation succeeded.
    pub const SUCCESS: i32 = 0;
    /// User error - bad arguments, invalid configuration.
    pub const USER_ERROR: i32 = 1;
    /// API error - server returned an error, operation failed.
    pub const API_ERROR: i32 = 2;
    /// Connection error - cannot reach the server.
    pub const CONNECTION_ERROR: i32 = 3;
}

/// CLI error with associated exit code.
#[derive(Debug)]
pub struct CliError {
    /// The underlying error.
    pub error: anyhow::Error,
    /// Exit code for this error.
    pub exit_code: i32,
    /// Optional hint for fixing the error.
    pub hint: Option<String>,
}

impl CliError {
    /// Create a new CLI error with the given exit code.
    pub fn new(error: impl Into<anyhow::Error>, exit_code: i32) -> Self {
        Self { error: error.into(), exit_code, hint: None }
    }

    /// Create a user error (exit code 1).
    pub fn user(error: impl Into<anyhow::Error>) -> Self {
        Self::new(error, exit_code::USER_ERROR)
    }

    /// Create an API error (exit code 2).
    pub fn api(error: impl Into<anyhow::Error>) -> Self {
        Self::new(error, exit_code::API_ERROR)
    }

    /// Create a connection error (exit code 3).
    pub fn connection(error: impl Into<anyhow::Error>) -> Self {
        Self::new(error, exit_code::CONNECTION_ERROR)
    }

    /// Add a hint to the error.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Format the error for human-readable output.
    pub fn format_human(&self) -> String {
        let mut output = format!("Error: {}", self.error);

        // Add hint if present
        if let Some(ref hint) = self.hint {
            output.push_str(&format!("\n\nHint: {hint}"));
        }

        output
    }

    /// Format the error for JSON output.
    pub fn format_json(&self) -> String {
        let mut obj = serde_json::json!({
            "error": {
                "message": self.error.to_string(),
                "exit_code": self.exit_code
            }
        });

        if let Some(ref hint) = self.hint {
            obj["error"]["hint"] = serde_json::Value::String(hint.clone());
        }

        // Try to pretty-print, fall back to compact
        serde_json::to_string_pretty(&obj).unwrap_or_else(|_| obj.to_string())
    }

    /// Exit the process with this error.
    pub fn exit(&self, json: bool) -> ! {
        if json {
            eprintln!("{}", self.format_json());
        } else {
            eprintln!("{}", self.format_human());
        }
        std::process::exit(self.exit_code);
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.error)
    }
}

impl std::error::Error for CliError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.error.source()
    }
}

/// Classify an error and return appropriate `CliError`.
pub fn classify_error(err: anyhow::Error) -> CliError {
    let err_str = err.to_string().to_lowercase();

    // Connection errors
    if err_str.contains("connection refused")
        || err_str.contains("dns error")
        || err_str.contains("name or service not known")
        || err_str.contains("network unreachable")
        || err_str.contains("no route to host")
        || err_str.contains("connect")
        || err_str.contains("timeout")
    {
        return CliError::connection(err)
            .with_hint("Check that the WAFER runtime is running and the endpoint URL is correct.");
    }

    // API errors (HTTP 4xx/5xx responses)
    if err_str.contains("status 4") || err_str.contains("status 5") {
        return CliError::api(err);
    }

    // Config/user errors (check before generic "not found" API errors)
    if err_str.contains("config") || err_str.contains("endpoint") || err_str.contains("parse") {
        return CliError::user(err);
    }

    // Check for specific API error messages
    if err_str.contains("not found")
        || err_str.contains("not swappable")
        || err_str.contains("not implemented")
        || err_str.contains("swap in progress")
    {
        return CliError::api(err);
    }

    // Default to API error for unclassified errors
    CliError::api(err)
}

/// Result type for CLI operations.
pub type Result<T> = std::result::Result<T, CliError>;

/// Extension trait for converting anyhow errors to CliError.
#[allow(dead_code)]
pub trait ResultExt<T> {
    /// Convert to CLI result with error classification.
    fn classify(self) -> Result<T>;

    /// Convert to user error.
    fn user_err(self) -> Result<T>;

    /// Convert to API error.
    fn api_err(self) -> Result<T>;

    /// Convert to connection error.
    fn conn_err(self) -> Result<T>;
}

impl<T> ResultExt<T> for anyhow::Result<T> {
    fn classify(self) -> Result<T> {
        self.map_err(classify_error)
    }

    fn user_err(self) -> Result<T> {
        self.map_err(CliError::user)
    }

    fn api_err(self) -> Result<T> {
        self.map_err(CliError::api)
    }

    fn conn_err(self) -> Result<T> {
        self.map_err(CliError::connection)
    }
}

// Implementations for common error types that aren't anyhow::Error

impl<T> ResultExt<T> for std::result::Result<T, serde_json::Error> {
    fn classify(self) -> Result<T> {
        self.map_err(|e| classify_error(e.into()))
    }

    fn user_err(self) -> Result<T> {
        self.map_err(CliError::user)
    }

    fn api_err(self) -> Result<T> {
        self.map_err(CliError::api)
    }

    fn conn_err(self) -> Result<T> {
        self.map_err(CliError::connection)
    }
}

impl<T> ResultExt<T> for std::result::Result<T, std::io::Error> {
    fn classify(self) -> Result<T> {
        self.map_err(|e| classify_error(e.into()))
    }

    fn user_err(self) -> Result<T> {
        self.map_err(CliError::user)
    }

    fn api_err(self) -> Result<T> {
        self.map_err(CliError::api)
    }

    fn conn_err(self) -> Result<T> {
        self.map_err(CliError::connection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exit_codes() {
        assert_eq!(exit_code::SUCCESS, 0);
        assert_eq!(exit_code::USER_ERROR, 1);
        assert_eq!(exit_code::API_ERROR, 2);
        assert_eq!(exit_code::CONNECTION_ERROR, 3);
    }

    #[test]
    fn test_error_classification_connection() {
        let err = anyhow::anyhow!("connection refused");
        let cli_err = classify_error(err);
        assert_eq!(cli_err.exit_code, exit_code::CONNECTION_ERROR);
        assert!(cli_err.hint.is_some());
    }

    #[test]
    fn test_error_classification_api() {
        let err = anyhow::anyhow!("node not found");
        let cli_err = classify_error(err);
        assert_eq!(cli_err.exit_code, exit_code::API_ERROR);
    }

    #[test]
    fn test_error_classification_user() {
        let err = anyhow::anyhow!("endpoint 'foo' not found in config");
        let cli_err = classify_error(err);
        assert_eq!(cli_err.exit_code, exit_code::USER_ERROR);
    }

    #[test]
    fn test_format_human() {
        let err = CliError::connection(anyhow::anyhow!("connection refused"))
            .with_hint("Check the server is running");
        let formatted = err.format_human();
        assert!(formatted.contains("connection refused"));
        assert!(formatted.contains("Hint:"));
    }

    #[test]
    fn test_format_json() {
        let err = CliError::api(anyhow::anyhow!("node not found"));
        let formatted = err.format_json();
        let parsed: serde_json::Value = serde_json::from_str(&formatted).unwrap();
        assert_eq!(parsed["error"]["exit_code"], 2);
        assert!(parsed["error"]["message"].as_str().unwrap().contains("not found"));
    }
}
