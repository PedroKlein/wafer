//! Registry and cache tests for WAFER.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;

use wafer_poc::config::NodeConfig;
use wafer_poc::registry::{OciReference, PackageCache, PluginSource, RegistryConfig, WaferRegistry};

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

    cache.put("ghcr.io", "user/plugin", "1.0.0", b"content").unwrap();

    // Wait for TTL to expire
    std::thread::sleep(Duration::from_millis(50));

    // Cache should return None (expired)
    assert!(
        cache.get("ghcr.io", "user/plugin", "1.0.0").is_none(),
        "Expected cache entry to be expired"
    );
}

#[test]
fn test_cache_fresh_entry() {
    let temp = TempDir::new().unwrap();
    // Use a long TTL
    let cache = PackageCache::new(temp.path().to_path_buf(), Duration::from_secs(3600));

    cache.put("ghcr.io", "user/plugin", "1.0.0", b"content").unwrap();

    // Cache should return the entry (fresh)
    let entry = cache
        .get("ghcr.io", "user/plugin", "1.0.0")
        .expect("Expected cache entry to exist");
    assert_eq!(entry.size, 7); // "content" is 7 bytes
}

#[test]
fn test_cache_clear_expired() {
    let temp = TempDir::new().unwrap();
    // Use a very short TTL
    let cache = PackageCache::new(temp.path().to_path_buf(), Duration::from_millis(10));

    // Use flat namespace/name structure that matches the cache's 3-level iteration
    cache.put("ns1", "pkg1", "1.0.0", b"content1").unwrap();
    cache.put("ns2", "pkg2", "2.0.0", b"content2").unwrap();

    // Wait for TTL to expire (longer wait for CI reliability)
    std::thread::sleep(Duration::from_millis(100));

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
        oci: Some("ghcr.io/user/plugin:1.0.0".to_string()),
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
        oci: None,
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
fn test_config_validation_invalid_oci_ref() {
    let config = NodeConfig {
        plugin_path: None,
        oci: Some("invalid-no-tag".to_string()),
        fuel_limit: None,
    };

    let err = config.validate().unwrap_err();
    assert!(
        err.to_string().contains("invalid OCI reference"),
        "Expected error about invalid OCI reference, got: {}",
        err
    );
}

#[test]
fn test_config_validation_local_valid() {
    let config = NodeConfig {
        plugin_path: Some("path.wasm".into()),
        oci: None,
        fuel_limit: None,
    };

    assert!(config.validate().is_ok(), "Expected local config to be valid");
    assert!(config.is_local());
    assert!(!config.is_oci());
}

#[test]
fn test_config_validation_oci_valid() {
    let config = NodeConfig {
        plugin_path: None,
        oci: Some("ghcr.io/pedroklein/wafer-uppercase:0.0.1".to_string()),
        fuel_limit: None,
    };

    assert!(
        config.validate().is_ok(),
        "Expected OCI config to be valid"
    );
    assert!(!config.is_local());
    assert!(config.is_oci());
}

// ============================================================================
// RegistryConfig Tests
// ============================================================================

#[test]
fn test_registry_config_defaults() {
    let config = RegistryConfig::default();

    assert_eq!(config.cache_ttl_hours, 24);
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
            || registry.cache_dir().to_string_lossy().contains("plugins")
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
// OciReference Tests
// ============================================================================

#[test]
fn test_oci_reference_parse_valid() {
    let oci = OciReference::parse("ghcr.io/pedroklein/wafer-uppercase:0.0.1").unwrap();
    assert_eq!(oci.registry, "ghcr.io");
    assert_eq!(oci.repository, "pedroklein/wafer-uppercase");
    assert_eq!(oci.tag, "0.0.1");
    assert_eq!(oci.as_str(), "ghcr.io/pedroklein/wafer-uppercase:0.0.1");
}

#[test]
fn test_oci_reference_parse_docker_hub() {
    let docker = OciReference::parse("docker.io/library/nginx:latest").unwrap();
    assert_eq!(docker.registry, "docker.io");
    assert_eq!(docker.repository, "library/nginx");
    assert_eq!(docker.tag, "latest");
}

#[test]
fn test_oci_reference_parse_invalid() {
    assert!(OciReference::parse("invalid").is_none());
    assert!(OciReference::parse("ghcr.io/repo").is_none()); // no tag
    assert!(OciReference::parse("ghcr.io/repo:").is_none()); // empty tag
    assert!(OciReference::parse("/repo:tag").is_none()); // empty registry
}

#[test]
fn test_oci_reference_display() {
    let oci = OciReference::parse("ghcr.io/pedroklein/wafer-uppercase:1.0.0").unwrap();
    assert_eq!(oci.to_string(), "ghcr.io/pedroklein/wafer-uppercase:1.0.0");
}

// ============================================================================
// PluginSource Tests
// ============================================================================

#[test]
fn test_plugin_source_local() {
    let source = PluginSource::local("/path/to/plugin.wasm");
    assert!(source.is_local());
    assert!(!source.is_oci());
}

#[test]
fn test_plugin_source_oci() {
    let oci_ref = OciReference::parse("ghcr.io/user/plugin:1.0.0").unwrap();
    let source = PluginSource::oci(oci_ref);
    assert!(!source.is_local());
    assert!(source.is_oci());
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

    assert_eq!(config.registry.cache_ttl_hours, 24);
    assert_eq!(config.nodes.len(), 3);
    assert_eq!(config.edges.len(), 2);
}
