//! Content-addressed workspace objects and rebuildable compact-pack indexes.

use crate::support::common::Timestamp;
use crate::support::error::{DraftError, DraftErrorKind, DraftResult};
use crate::support::fsutil::{write_atomic, write_json};
use crate::support::hashing::{blake3_hex, hex_decode};
use crate::workspace::layout::DraftLayout;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObjectPackIndex {
    pub(crate) schema_version: u32,
    pub(crate) objects: BTreeMap<String, String>,
}

impl crate::contracts::VersionedContract for ObjectPackIndex {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::ObjectPackIndex;
}

impl Default for ObjectPackIndex {
    fn default() -> Self {
        Self {
            schema_version: crate::contracts::current_version(
                crate::contracts::ContractId::ObjectPackIndex,
            ),
            objects: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObjectPack {
    pub(crate) schema_version: u32,
    pub(crate) id: String,
    pub(crate) created_at: Timestamp,
    pub(crate) entries: Vec<ObjectPackEntry>,
}

impl crate::contracts::VersionedContract for ObjectPack {
    const CONTRACT: crate::contracts::ContractId = crate::contracts::ContractId::ObjectPack;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObjectPackEntry {
    pub(crate) object_ref: String,
    pub(crate) compressed_hex: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ObjectStore {
    layout: DraftLayout,
}

impl ObjectStore {
    pub(crate) fn new(layout: DraftLayout) -> Self {
        Self { layout }
    }

    pub(crate) fn put_bytes(&self, data: &[u8]) -> DraftResult<String> {
        let hash = blake3_hex(data);
        let (prefix, rest) = hash.split_at(2);
        let path = self.layout.objects_dir().join(prefix).join(rest);
        if !path.exists() {
            let compressed = zstd::stream::encode_all(data, 3).map_err(|error| {
                DraftError::storage(format!("zstd compression failed: {error}"))
            })?;
            write_atomic(&path, &compressed)?;
        }
        Ok(format!("b3:{hash}"))
    }

    pub(crate) fn get_bytes(&self, object_ref: &str) -> DraftResult<Vec<u8>> {
        let hash = object_ref.strip_prefix("b3:").ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!("unsupported object reference '{object_ref}'"),
            )
        })?;
        if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!("invalid object reference '{object_ref}'"),
            ));
        }
        let (prefix, rest) = hash.split_at(2);
        let loose_path = self.layout.objects_dir().join(prefix).join(rest);
        let compressed = if loose_path.exists() {
            fs::read(loose_path)?
        } else {
            self.get_packed_bytes(object_ref)?
        };
        let data = zstd::stream::decode_all(compressed.as_slice())
            .map_err(|error| DraftError::storage(format!("zstd decompression failed: {error}")))?;
        let actual = blake3_hex(&data);
        if actual != hash {
            return Err(DraftError::new(
                DraftErrorKind::CorruptData,
                format!("object hash mismatch for {object_ref}: expected {hash}, got {actual}"),
            ));
        }
        Ok(data)
    }

    fn get_packed_bytes(&self, object_ref: &str) -> DraftResult<Vec<u8>> {
        let index = read_object_pack_index(&self.layout)?;
        let pack_name = index.objects.get(object_ref).ok_or_else(|| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!("object {object_ref} is missing from loose and compact storage"),
            )
        })?;
        let pack_path = self.layout.object_packs_dir().join(pack_name);
        let pack_bytes = fs::read(&pack_path)?;
        let json = zstd::stream::decode_all(pack_bytes.as_slice()).map_err(|error| {
            DraftError::new(
                DraftErrorKind::CorruptData,
                format!(
                    "object pack decompression failed for {}: {error}",
                    pack_path.display()
                ),
            )
        })?;
        let pack: ObjectPack = crate::contracts::decode_persisted(&json)?;
        let entry = pack
            .entries
            .into_iter()
            .find(|entry| entry.object_ref == object_ref)
            .ok_or_else(|| {
                DraftError::new(
                    DraftErrorKind::CorruptData,
                    format!("object {object_ref} is absent from indexed pack {pack_name}"),
                )
            })?;
        hex_decode(&entry.compressed_hex)
    }
}

pub(crate) fn read_object_pack_index(layout: &DraftLayout) -> DraftResult<ObjectPackIndex> {
    let path = layout.object_packs_dir().join("index.json");
    if !path.exists() {
        return Ok(ObjectPackIndex::default());
    }
    crate::contracts::read_persisted(&path)
}

pub(crate) fn write_object_pack_index(
    layout: &DraftLayout,
    index: &ObjectPackIndex,
) -> DraftResult<()> {
    write_json(&layout.object_packs_dir().join("index.json"), index)
}
