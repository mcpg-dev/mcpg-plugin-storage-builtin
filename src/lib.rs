//! Built-in `ContentStorePlugin` factories — `in_process` + `file_system`.
//!
//! These are the two backends every gateway build ships: zero
//! external deps, zero operator-side opt-in. Operators reference
//! them in the top-level `storage.providers:` list by
//! `kind: in_process` or `kind: file_system`. Third-party storage
//! backends (S3, GCS, etc.) ship as separate plugins.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use mcpg_backend_llm_shared::{
    ContentStore, ContentStoreError, ContentStorePlugin, FileSystemContentStore,
    InProcessContentStore,
};
use mcpg_plugin_protocol::PluginManifest;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Manifests
// ---------------------------------------------------------------------------

const PLUGIN_VERSION: &str = env!("CARGO_PKG_VERSION");
const PROTOCOL_VERSION: &str = "1.0";

fn in_process_manifest() -> PluginManifest {
    PluginManifest {
        id: "dev.mcpg.storage.in_process".into(),
        version: PLUGIN_VERSION.into(),
        name: "In-Process Content Store".into(),
        plugin_class: mcpg_plugin_protocol::PluginClass::ContentStore,
        protocol_version: PROTOCOL_VERSION.into(),
        license: None,
        required_capabilities: Vec::new(),
        tags: vec!["builtin".into(), "ephemeral".into()],
        provides: Vec::new(),
        provides_schemes: Vec::new(),
        module_path_prefix: ::std::module_path!()
            .split("::")
            .next()
            .unwrap_or("")
            .to_owned(),
        backend_profile: None,
    }
}

fn file_system_manifest() -> PluginManifest {
    PluginManifest {
        id: "dev.mcpg.storage.file_system".into(),
        version: PLUGIN_VERSION.into(),
        name: "File-System Content Store".into(),
        plugin_class: mcpg_plugin_protocol::PluginClass::ContentStore,
        protocol_version: PROTOCOL_VERSION.into(),
        license: None,
        required_capabilities: Vec::new(),
        tags: vec!["builtin".into(), "persistent".into()],
        provides: Vec::new(),
        provides_schemes: Vec::new(),
        module_path_prefix: ::std::module_path!()
            .split("::")
            .next()
            .unwrap_or("")
            .to_owned(),
        backend_profile: None,
    }
}

// ---------------------------------------------------------------------------
// In-process plugin
// ---------------------------------------------------------------------------

/// `type: in_process` — `Arc<RwLock<LruCache<…>>>`. Volatile, lost
/// on restart. Default for single-node deployments without
/// persistence requirements.
#[derive(Debug)]
pub struct InProcessStoragePlugin {
    manifest: PluginManifest,
}

impl Default for InProcessStoragePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl InProcessStoragePlugin {
    pub fn new() -> Self {
        Self {
            manifest: in_process_manifest(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InProcessSpec {
    /// Aggregate byte size cap. `0` = unlimited (TTL still applies).
    /// Default 256 MiB.
    #[serde(default = "InProcessSpec::default_max_bytes")]
    max_bytes: usize,
}

impl InProcessSpec {
    fn default_max_bytes() -> usize {
        256 * 1024 * 1024
    }
}

impl Default for InProcessSpec {
    fn default() -> Self {
        Self {
            max_bytes: Self::default_max_bytes(),
        }
    }
}

#[async_trait]
impl ContentStorePlugin for InProcessStoragePlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn kind(&self) -> &str {
        "in_process"
    }

    async fn build_profile(
        &self,
        _profile_name: &str,
        spec: &serde_json::Value,
    ) -> Result<Arc<dyn ContentStore>, ContentStoreError> {
        let parsed: InProcessSpec =
            if spec.is_null() || spec.as_object().is_some_and(|o| o.is_empty()) {
                InProcessSpec::default()
            } else {
                serde_json::from_value(spec.clone()).map_err(|e| ContentStoreError::Storage {
                    message: format!("invalid in_process spec: {e}"),
                })?
            };
        Ok(InProcessContentStore::new(parsed.max_bytes))
    }
}

// ---------------------------------------------------------------------------
// Filesystem plugin
// ---------------------------------------------------------------------------

/// `type: file_system` — on-disk store under operator-supplied
/// `root`. Survives restart; same-host only (multi-replica gateways
/// need S3 or another shared backend).
#[derive(Debug)]
pub struct FileSystemStoragePlugin {
    manifest: PluginManifest,
}

impl Default for FileSystemStoragePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FileSystemStoragePlugin {
    pub fn new() -> Self {
        Self {
            manifest: file_system_manifest(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileSystemSpec {
    /// Host filesystem path. Created if missing. Must be writable.
    root: PathBuf,
    /// Aggregate blob byte size cap. `0` = unlimited. Default 8 GiB.
    #[serde(default = "FileSystemSpec::default_max_bytes")]
    max_bytes: u64,
}

impl FileSystemSpec {
    fn default_max_bytes() -> u64 {
        8 * 1024 * 1024 * 1024
    }
}

#[async_trait]
impl ContentStorePlugin for FileSystemStoragePlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn kind(&self) -> &str {
        "file_system"
    }

    async fn build_profile(
        &self,
        _profile_name: &str,
        spec: &serde_json::Value,
    ) -> Result<Arc<dyn ContentStore>, ContentStoreError> {
        let parsed: FileSystemSpec =
            serde_json::from_value(spec.clone()).map_err(|e| ContentStoreError::Storage {
                message: format!("invalid file_system spec: {e}"),
            })?;
        let store = FileSystemContentStore::open(parsed.root, parsed.max_bytes).await?;
        Ok(store)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_process_plugin_builds_default_instance() {
        let plugin = InProcessStoragePlugin::new();
        assert_eq!(plugin.kind(), "in_process");
        let store = plugin
            .build_profile("default", &serde_json::json!({}))
            .await
            .unwrap();
        let stats = store.stats();
        assert_eq!(stats.max_bytes, 256 * 1024 * 1024);
    }

    #[tokio::test]
    async fn in_process_plugin_honours_max_bytes() {
        let plugin = InProcessStoragePlugin::new();
        let store = plugin
            .build_profile("small", &serde_json::json!({"max_bytes": 1024}))
            .await
            .unwrap();
        assert_eq!(store.stats().max_bytes, 1024);
    }

    #[tokio::test]
    async fn in_process_plugin_rejects_unknown_field() {
        let plugin = InProcessStoragePlugin::new();
        let err = plugin
            .build_profile("x", &serde_json::json!({"bogus": true}))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ContentStoreError::Storage { .. }),
            "expected Storage error, got {err:?}"
        );
    }

    #[tokio::test]
    async fn file_system_plugin_builds_instance_and_persists() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin = FileSystemStoragePlugin::new();
        assert_eq!(plugin.kind(), "file_system");
        let store = plugin
            .build_profile(
                "media",
                &serde_json::json!({
                    "root": tmp.path(),
                    "max_bytes": 4096
                }),
            )
            .await
            .unwrap();
        let h = store
            .put(mcpg_backend_llm_shared::ContentToStore {
                bytes: bytes::Bytes::from_static(b"hello"),
                mime_type: "text/plain".into(),
                alias: None,
                session_id: None,
                tenant_id: None,
                ttl: None,
            })
            .await
            .unwrap();
        let got = store.get(&h.id).await.unwrap().unwrap();
        assert_eq!(got.bytes.as_ref(), b"hello");
    }

    #[tokio::test]
    async fn file_system_plugin_requires_root() {
        let plugin = FileSystemStoragePlugin::new();
        let err = plugin
            .build_profile("x", &serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, ContentStoreError::Storage { .. }));
    }

    #[test]
    fn manifests_classify_as_content_store() {
        let in_proc = in_process_manifest();
        let fs = file_system_manifest();
        assert!(matches!(
            in_proc.plugin_class,
            mcpg_plugin_protocol::PluginClass::ContentStore
        ));
        assert!(matches!(
            fs.plugin_class,
            mcpg_plugin_protocol::PluginClass::ContentStore
        ));
        assert!(in_proc.tags.contains(&"builtin".to_owned()));
        assert!(fs.tags.contains(&"builtin".to_owned()));
    }
}
