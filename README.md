# mcpg-plugin-storage-builtin

> The two content-store backends every MCPG gateway build ships: an in-memory LRU store and an on-disk store.

MCPG gateways hand large tool results, generated images, audio, and other blobs
to a **content store** instead of inlining them in an MCP response, then serve
them back through `mcpg-resource://` URIs. This crate provides the two stores
that are always present — `in_process` and `file_system` — as
`ContentStorePlugin` factories, so operators always have somewhere for bytes to
live without installing anything. It is deliberately dependency-free at the
storage layer: it holds no client libraries and reaches no network. Backends
that talk to an object store, such as `mcpg-plugin-storage-s3`, are separate
crates that implement the same trait.

## What's here
- `InProcessStoragePlugin` — factory for `kind = "in_process"`, building an
  `InProcessContentStore`: an LRU map behind a lock, capped by `max_bytes`,
  expiring lazily on read and in a periodic sweep. Volatile; contents are lost
  when the process exits.
- `FileSystemStoragePlugin` — factory for `kind = "file_system"`, building a
  `FileSystemContentStore` rooted at an operator path. Blobs are written under
  a BLAKE3-derived path with a JSON metadata sidecar and an alias record,
  each written to a temporary file and renamed into place so a crash never
  leaves a half-written object. An in-memory index is rebuilt by walking the
  metadata tree at startup and drives lookup, `stats()`, and LRU eviction once
  `max_bytes` is exceeded.
- Both implement `mcpg_backend_llm_shared::ContentStorePlugin` — `manifest()`,
  `kind()`, and an async `build_profile(profile_name, spec)` that validates the
  operator's `config:` object and returns an `Arc<dyn ContentStore>`.
- Neither store vends presigned URLs; `signed_url` returns
  `ContentStoreError::SignedUrlNotSupported`.

This crate is not a cdylib plugin. It has no `plugin.yaml`, exports no
`mcpg_plugin_register` symbol, and is never signed, packed, or pulled from a
registry — it is linked into the gateway binary directly.

## Used by
- The MCPG gateway, which registers both factories unconditionally while
  building its content-store registry and dispatches `storage.providers[]`
  entries to them by `kind`.
- Anything embedding the MCPG content-store traits that wants a working default
  store without adding an object-storage dependency.

## Usage

### As a Rust dependency

```toml
[dependencies]
mcpg-plugin-storage-builtin = "<version>"
mcpg-backend-llm-shared = "<version>"
serde_json = "1"
```

```rust
use std::sync::Arc;

use mcpg_backend_llm_shared::{ContentStore, ContentStoreError, ContentStorePlugin};
use mcpg_plugin_storage_builtin::{FileSystemStoragePlugin, InProcessStoragePlugin};

async fn open_stores() -> Result<Vec<Arc<dyn ContentStore>>, ContentStoreError> {
    let scratch: Arc<dyn ContentStorePlugin> = Arc::new(InProcessStoragePlugin::new());
    let durable: Arc<dyn ContentStorePlugin> = Arc::new(FileSystemStoragePlugin::new());

    Ok(vec![
        scratch
            .build_profile("scratch", &serde_json::json!({ "max_bytes": 64 * 1024 * 1024 }))
            .await?,
        durable
            .build_profile(
                "media",
                &serde_json::json!({ "root": "/var/lib/mcpg/content" }),
            )
            .await?,
    ])
}
```

### Operator configuration

Both stores are selected from the gateway's dedicated top-level `storage:`
block, by `kind`. There is no `plugins:` entry to add — the factories are
already registered. Bindings route to a provider through their own
`content_storage:` field, and `storage.default` names the provider used by
bindings that do not.

```yaml
storage:
  default: scratch
  providers:
    - id: scratch
      kind: in_process
      config:
        max_bytes: 268435456          # 256 MiB
    - id: media
      kind: file_system
      config:
        root: /var/lib/mcpg/content
        max_bytes: 8589934592         # 8 GiB
```

`in_process` configuration:

| Field | Type | Default | Description |
|---|---|---|---|
| `max_bytes` | integer | `268435456` (256 MiB) | Aggregate byte cap; `0` means unlimited, with TTL expiry still applied. |

`file_system` configuration:

| Field | Type | Default | Description |
|---|---|---|---|
| `root` | path | *(required)* | Directory blobs are written under. Created if missing; must be writable. |
| `max_bytes` | integer | `8589934592` (8 GiB) | Aggregate byte cap; `0` means unlimited. |

Both reject unknown fields, and an invalid spec aborts gateway boot rather than
starting with an unusable store. When `storage.providers` is empty and no
binding names a provider, the gateway creates a single `in_process` provider
with the id `default` and the standard 256 MiB cap.

Choose `file_system` when content must survive a restart. It is bound to one
host's disk, so a multi-replica deployment needs a shared backend such as
`mcpg-plugin-storage-s3` instead.

## Build / test
```bash
cargo build -p mcpg-plugin-storage-builtin
cargo test  -p mcpg-plugin-storage-builtin
```

## See also
- Full gateway config schema, including the `storage:` block: <https://mcpg.dev/docs/reference/configuration>
- Plugin classes and the plugin ABI: <https://mcpg.dev/docs/plugins/plugins-and-protocol>
- `mcpg-plugin-storage-s3` — the S3-compatible store for shared, cross-replica
  content.
