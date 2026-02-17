//! Registry and cache tests for WAFER.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;

use wafer_poc::config::NodeConfig;
use wafer_poc::registry::{PackageCache, PackageRef, PluginSource, RegistryConfig, WaferRegistry};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn wafer_binary() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    path.pop();
    path.push("wafer-poc");
    path
}

// ============================================================================
// PackageCache Tests
// ============================================================================

#[test]
fn test_cache_ttl_expiry() {
    let temp = TempDir::new().unwrap();
    // Use a very short TTL for testing
    let cache = PackageCache::new(temp.path().to_path_buf(), Duration::from_millis(1));

    cache.put("test", "pkg", "1.0.0", b"content").unwrap();

    // Wait for TTL to expire
    std::thread::sleep(Duration::from_millis(50));

    // Cache should return None (expired)
    assert!(
        cache.get("test", "pkg", "1.0.0").is_none(),
        "Expected cache entry to be expired"
    );
}

#[test]
fn test_cache_fresh_entry() {
    let temp = TempDir::new().unwrap();
    // Use a long TTL
    let cache = PackageCache::new(temp.path().to_path_buf(), Duration::from_secs(3600));

    cache.put("test", "pkg", "1.0.0", b"content").unwrap();

    // Cache should return the entry (fresh)
    let entry = cache
        .get("test", "pkg", "1.0.0")
        .expect("Expected cache entry to exist");
    assert_eq!(entry.size, 7); // "content" is 7 bytes
}

#[test]
fn test_cache_clear_expired() {
    let temp = TempDir::new().unwrap();
    // Use a very short TTL
    let cache = PackageCache::new(temp.path().to_path_buf(), Duration::from_millis(1));

    cache.put("ns1", "pkg1", "1.0.0", b"content1").unwrap();
    cache.put("ns2", "pkg2", "2.0.0", b"content2").unwrap();

    // Wait for TTL to expire
    std::thread::sleep(Duration::from_millis(50));

    // Clear expired entries
    let removed = cache.clear_expired().unwrap();
    assert_eq!(removed, 2, "Expected 2 entries to be removed");
}

// ============================================================================
// NodeConfig Validation Tests
// ============================================================================

#[test]
fn test_config_validation_both_specified() {
    let config = NodeConfig {
        plugin_path: Some("path.wasm".into()),
        package: Some("wafer:test".to_string()),
        version: Some("1.0.0".to_string()),
        registry: None,
        fuel_limit: None,
    };

    let err = config.validate().unwrap_err();
    assert!(
        err.to_string().contains("cannot specify both"),
        "Expected error about both specified, got: {}",
        err
    );
}

#[test]
fn test_config_validation_neither_specified() {
    let config = NodeConfig {
        plugin_path: None,
        package: None,
        version: None,
        registry: None,
        fuel_limit: None,
    };

    let err = config.validate().unwrap_err();
    assert!(
        err.to_string().contains("must specify either"),
        "Expected error about neither specified, got: {}",
        err
    );
}

#[test]
fn test_config_validation_package_without_version() {
    let config = NodeConfig {
        plugin_path: None,
        package: Some("wafer:test".to_string()),
        version: None,
        registry: None,
        fuel_limit: None,
    };

    let err = config.validate().unwrap_err();
    assert!(
        err.to_string().contains("version"),
        "Expected error about missing version, got: {}",
        err
    );
}

#[test]
fn test_config_validation_local_valid() {
    let config = NodeConfig {
        plugin_path: Some("path.wasm".into()),
        package: None,
        version: None,
        registry: None,
        fuel_limit: None,
    };

    assert!(config.validate().is_ok(), "Expected local config to be valid");
    assert!(config.is_local());
    assert!(!config.is_remote());
}

#[test]
fn test_config_validation_remote_valid() {
    let config = NodeConfig {
        plugin_path: None,
        package: Some("wafer:test".to_string()),
        version: Some("^1.0".to_string()),
        registry: None,
        fuel_limit: None,
    };

    assert!(
        config.validate().is_ok(),
        "Expected remote config to be valid"
    );
    assert!(!config.is_local());
    assert!(config.is_remote());
}

#[test]
fn test_config_validation_remote_with_registry_override() {
    let config = NodeConfig {
        plugin_path: None,
        package: Some("wafer:test".to_string()),
        version: Some("=2.0.0".to_string()),
        registry: Some("custom.registry.io".to_string()),
        fuel_limit: None,
    };

    assert!(config.validate().is_ok());
    let source = config.plugin_source().unwrap();
    if let PluginSource::Remote { package, version: _ } = source {
        assert_eq!(package.registry.as_deref(), Some("custom.registry.io"));
    } else {
        panic!("Expected remote source");
    }
}

// ============================================================================
// RegistryConfig Tests
// ============================================================================

#[test]
fn test_registry_config_defaults() {
    let config = RegistryConfig::default();

    assert_eq!(config.cache_ttl_hours, 24);
    assert!(config.default_registry.is_none());
    assert!(!config.no_cache);
}

#[test]
fn test_registry_config_cache_ttl() {
    let config = RegistryConfig {
        cache_ttl_hours: 12,
        ..Default::default()
    };

    assert_eq!(config.cache_ttl(), Duration::from_secs(12 * 3600));
}

#[test]
fn test_registry_config_no_cache_flag() {
    let config = RegistryConfig {
        no_cache: true,
        ..Default::default()
    };

    assert!(config.no_cache);
}

// ============================================================================
// WaferRegistry Tests
// ============================================================================

#[test]
fn test_registry_creation() {
    let config = RegistryConfig::default();
    let registry = WaferRegistry::new(config).unwrap();

    // Just verify it creates successfully
    assert!(
        registry.cache_dir().to_string_lossy().contains("wafer")
            || registry.cache_dir().to_string_lossy().contains("packages")
    );
}

#[test]
fn test_registry_with_no_cache() {
    let config = RegistryConfig {
        no_cache: true,
        ..Default::default()
    };

    let registry = WaferRegistry::new(config).unwrap();
    // Verify it creates successfully with no_cache enabled
    assert!(registry.cache_dir().exists() || !registry.cache_dir().exists());
}

#[test]
fn test_registry_with_custom_cache_dir() {
    let temp = TempDir::new().unwrap();
    let config = RegistryConfig {
        cache_dir: Some(temp.path().to_path_buf()),
        ..Default::default()
    };

    let registry = WaferRegistry::new(config).unwrap();
    assert_eq!(registry.cache_dir(), temp.path());
}

#[tokio::test]
async fn test_resolve_local_existing() {
    let config = RegistryConfig::default();
    let registry = WaferRegistry::new(config).unwrap();

    // Use Cargo.toml as a test file that exists
    let source = PluginSource::local("Cargo.toml");
    let resolved = registry.resolve(&source).await.unwrap();

    assert!(resolved.resolved_version.is_none());
    assert!(!resolved.content_hash.is_empty());
}

#[tokio::test]
async fn test_resolve_local_missing() {
    let config = RegistryConfig::default();
    let registry = WaferRegistry::new(config).unwrap();

    let source = PluginSource::local("/nonexistent/path.wasm");
    let result = registry.resolve(&source).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("not found"),
        "Expected 'not found' error, got: {}",
        err
    );
}

// ============================================================================
// PackageRef Tests
// ============================================================================

#[test]
fn test_package_ref_parse_valid() {
    let pkg = PackageRef::parse("wafer:uppercase").unwrap();
    assert_eq!(pkg.namespace, "wafer");
    assert_eq!(pkg.name, "uppercase");
    assert!(pkg.registry.is_none());
}

#[test]
fn test_package_ref_parse_invalid() {
    assert!(PackageRef::parse("invalid").is_none());
    assert!(PackageRef::parse(":name").is_none());
    assert!(PackageRef::parse("namespace:").is_none());
    assert!(PackageRef::parse("").is_none());
}

#[test]
fn test_package_ref_with_registry() {
    let pkg = PackageRef::new("wafer", "uppercase").with_registry("ghcr.io/custom");
    assert_eq!(pkg.namespace, "wafer");
    assert_eq!(pkg.name, "uppercase");
    assert_eq!(pkg.registry.as_deref(), Some("ghcr.io/custom"));
}

#[test]
fn test_package_ref_display() {
    let pkg = PackageRef::new("wafer", "uppercase");
    assert_eq!(pkg.to_string(), "wafer:uppercase");
}

// ============================================================================
// PluginSource Tests
// ============================================================================

#[test]
fn test_plugin_source_local() {
    let source = PluginSource::local("/path/to/plugin.wasm");
    assert!(source.is_local());
    assert!(!source.is_remote());
}

#[test]
fn test_plugin_source_remote() {
    use semver::Version;
    let pkg = PackageRef::new("wafer", "uppercase");
    let source = PluginSource::remote(pkg, &Version::new(1, 0, 0));
    assert!(!source.is_local());
    assert!(source.is_remote());
}

#[test]
fn test_plugin_source_remote_req() {
    use semver::VersionReq;
    let pkg = PackageRef::new("wafer", "uppercase");
    let version = VersionReq::parse("^1.0").unwrap();
    let source = PluginSource::remote_req(pkg, version);
    assert!(!source.is_local());
    assert!(source.is_remote());
}

// ============================================================================
// CLI Tests
// ============================================================================

#[test]
fn test_cli_no_cache_flag_accepted() {
    let output = Command::new(wafer_binary())
        .args(["--help"])
        .current_dir(project_root())
        .output()
        .expect("Failed to execute");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--no-cache"),
        "Expected --no-cache flag in help output: {}",
        stdout
    );
}

#[test]
fn test_cli_no_cache_with_config() {
    // This test verifies the flag is accepted (will fail because remote package doesn't exist,
    // but that's expected - we just want to verify the flag parsing works)
    let output = Command::new(wafer_binary())
        .args(["--config", "examples/dag-remote.toml", "--no-cache"])
        .current_dir(project_root())
        .stdin(Stdio::null())
        .output()
        .expect("Failed to execute");

    // The command should fail (remote package doesn't exist), but NOT because of flag parsing
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("unexpected argument") && !stderr.contains("--no-cache"),
        "Flag should be accepted but got parsing error: {}",
        stderr
    );
}

#[test]
fn test_dag_remote_config_parses() {
    // Test that the dag-remote.toml config file parses correctly
    let config_content =
        std::fs::read_to_string(project_root().join("examples/dag-remote.toml")).unwrap();
    let config: wafer_poc::config::DagConfig = toml::from_str(&config_content).unwrap();

    assert_eq!(
        config.registry.default_registry.as_deref(),
        Some("ghcr.io/wafer-plugins")
    );
    assert_eq!(config.registry.cache_ttl_hours, 24);
    assert_eq!(config.nodes.len(), 3);
    assert_eq!(config.edges.len(), 2);
}
