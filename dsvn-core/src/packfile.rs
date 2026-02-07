//! Packfile storage (Git-style warm storage)
//!
//! Provides compressed storage for medium-age objects

use crate::object::ObjectId;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::{self, Read, Seek, Write};
use std::path::Path;
use zstd::stream::{decode_all as zstd_decode, encode_all as zstd_encode};

/// Packfile version
pub const PACK_VERSION: u32 = 2;

/// Packfile entry
#[derive(Debug, Clone)]
pub struct PackEntry {
    pub object_id: ObjectId,
    pub offset: u64,
    pub size: u64,
}

/// Packfile index
#[derive(Debug, Clone)]
pub struct PackIndex {
    pub entries: Vec<PackEntry>,
}

/// Packfile writer
pub struct PackWriter {
    objects: HashMap<ObjectId, Vec<u8>>,
}

/// Packfile reader
pub struct PackReader {
    index: PackIndex,
    data: Vec<u8>,
}

impl PackWriter {
    /// Create a new packfile writer
    pub fn create() -> Result<Self> {
        Ok(Self {
            objects: HashMap::new(),
        })
    }

    /// Add object to pack
    pub fn add_object(&mut self, id: ObjectId, data: Vec<u8>) {
        self.objects.insert(id, data);
    }

    /// Write packfile to disk
    pub fn write(&self, path: &Path) -> Result<PackIndex> {
        // Create parent directory
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .context("Failed to create packfile directory")?;
        }

        let mut file = std::fs::File::create(path)
            .context("Failed to create packfile")?;

        // Write header
        file.write_all(&PACK_VERSION.to_le_bytes())
            .context("Failed to write header")?;
        file.write_all(&(self.objects.len() as u32).to_le_bytes())
            .context("Failed to write object count")?;

        let mut entries = Vec::new();
        let mut current_offset = 8u64; // Header is 8 bytes

        // Write objects
        for (id, data) in &self.objects {
            let entry_start = current_offset;

            // Write object header (type + size)
            file.write_all(&[1u8]) // Type: blob
                .context("Failed to write object type")?;
            file.write_all(&(data.len() as u32).to_le_bytes())
                .context("Failed to write object size")?;
            file.write_all(id.as_bytes())
                .context("Failed to write object ID")?;

            // Compress and write data
            let compressed = zstd_encode(&data[..], 0)
                .context("Failed to compress data")?;
            file.write_all(&(compressed.len() as u32).to_le_bytes())
                .context("Failed to write compressed size")?;
            file.write_all(&compressed)
                .context("Failed to write compressed data")?;

            current_offset = file.stream_position()
                .context("Failed to get file position")?;

            entries.push(PackEntry {
                object_id: *id,
                offset: entry_start,
                size: data.len() as u64,
            });
        }

        Ok(PackIndex { entries })
    }
}

impl PackReader {
    /// Open an existing packfile
    pub fn open(path: &Path) -> Result<Self> {
        // Read packfile data
        let mut file = std::fs::File::open(path)
            .context("Failed to open packfile")?;
        let metadata = file.metadata()
            .context("Failed to read packfile metadata")?;
        let size = metadata.len() as usize;

        let mut data = vec![0u8; size];
        file.read_exact(&mut data)
            .context("Failed to read packfile")?;

        // Parse header
        let mut pos = 0;

        let version = u32::from_le_bytes(data[pos..pos+4].try_into().unwrap());
        pos += 4;
        if version != PACK_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Unknown pack version: {}", version),
            )
            .into());
        }

        let object_count = u32::from_le_bytes(data[pos..pos+4].try_into().unwrap()) as usize;
        pos += 4;

        // Read entries (scan through the file)
        let mut entries = Vec::new();
        for _ in 0..object_count {
            let entry_start = pos as u64;

            // Skip object type
            pos += 1;

            // Read object size
            let obj_size = u32::from_le_bytes(data[pos..pos+4].try_into().unwrap()) as u64;
            pos += 4;

            // Read object ID
            let mut id_bytes = [0u8; 32];
            id_bytes.copy_from_slice(&data[pos..pos+32]);
            let object_id = ObjectId::new(id_bytes);
            pos += 32;

            // Skip compressed data size and data
            let compressed_size = u32::from_le_bytes(data[pos..pos+4].try_into().unwrap()) as usize;
            pos += 4;
            pos += compressed_size;

            entries.push(PackEntry {
                object_id,
                offset: entry_start,
                size: obj_size,
            });
        }

        Ok(Self {
            index: PackIndex { entries },
            data,
        })
    }

    /// Get object from packfile
    pub fn get_object(&self, id: ObjectId) -> Result<Option<Vec<u8>>> {
        // Find entry
        let entry = match self.index.entries.iter().find(|e| e.object_id == id) {
            Some(e) => e,
            None => return Ok(None),
        };

        let mut pos = entry.offset as usize;

        // Skip object type
        pos += 1;

        // Skip object size
        pos += 4;

        // Skip object ID
        pos += 32;

        // Read compressed data size
        let compressed_size = u32::from_le_bytes(self.data[pos..pos+4].try_into().unwrap()) as usize;
        pos += 4;

        // Read and decompress data
        let compressed_data = &self.data[pos..pos+compressed_size];
        let decompressed = zstd_decode(compressed_data)
            .context("Failed to decompress object")?;

        Ok(Some(decompressed.to_vec()))
    }

    /// Get all object IDs
    pub fn object_ids(&self) -> Vec<ObjectId> {
        self.index.entries.iter().map(|e| e.object_id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_packfile_write_and_read() {
        let temp_dir = TempDir::new().unwrap();
        let pack_path = temp_dir.path().join("test.pack");

        // Create packfile
        let mut writer = PackWriter::create().unwrap();

        let id1 = ObjectId::from_data(b"hello world");
        let data1 = b"hello world".to_vec();
        writer.add_object(id1, data1);

        let id2 = ObjectId::from_data(b"test data");
        let data2 = b"test data".to_vec();
        writer.add_object(id2, data2);

        let index = writer.write(&pack_path).unwrap();

        assert_eq!(index.entries.len(), 2);

        // Read packfile
        let reader = PackReader::open(&pack_path).unwrap();

        let retrieved1 = reader.get_object(id1).unwrap().unwrap();
        assert_eq!(retrieved1, b"hello world");

        let retrieved2 = reader.get_object(id2).unwrap().unwrap();
        assert_eq!(retrieved2, b"test data");
    }

    #[test]
    fn test_packfile_get_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let pack_path = temp_dir.path().join("test.pack");

        let mut writer = PackWriter::create().unwrap();
        let id = ObjectId::from_data(b"test");
        writer.add_object(id, b"test".to_vec());
        writer.write(&pack_path).unwrap();

        let reader = PackReader::open(&pack_path).unwrap();
        let nonexistent = ObjectId::from_data(b"nonexistent");

        let result = reader.get_object(nonexistent).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_packfile_object_ids() {
        let temp_dir = TempDir::new().unwrap();
        let pack_path = temp_dir.path().join("test.pack");

        let mut writer = PackWriter::create().unwrap();

        let id1 = ObjectId::from_data(b"object1");
        writer.add_object(id1, b"object1".to_vec());

        let id2 = ObjectId::from_data(b"object2");
        writer.add_object(id2, b"object2".to_vec());

        writer.write(&pack_path).unwrap();

        let reader = PackReader::open(&pack_path).unwrap();
        let ids = reader.object_ids();

        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&id1));
        assert!(ids.contains(&id2));
    }

    #[test]
    fn test_packfile_compression() {
        let temp_dir = TempDir::new().unwrap();
        let pack_path = temp_dir.path().join("test.pack");

        let mut writer = PackWriter::create().unwrap();

        // Large data that compresses well
        let large_data = vec![b'A'; 10_000];
        let id = ObjectId::from_data(&large_data);
        writer.add_object(id, large_data.clone());
        writer.write(&pack_path).unwrap();

        // Check file size is smaller than original
        let file_size = std::fs::metadata(&pack_path).unwrap().len();
        assert!(file_size < large_data.len() as u64 + 100); // +100 for headers

        // Verify decompression works
        let reader = PackReader::open(&pack_path).unwrap();
        let retrieved = reader.get_object(id).unwrap().unwrap();
        assert_eq!(retrieved, large_data);
    }

    #[test]
    fn test_packfile_empty() {
        let temp_dir = TempDir::new().unwrap();
        let pack_path = temp_dir.path().join("test.pack");

        let writer = PackWriter::create().unwrap();
        let index = writer.write(&pack_path).unwrap();

        assert_eq!(index.entries.len(), 0);

        let reader = PackReader::open(&pack_path).unwrap();
        assert_eq!(reader.object_ids().len(), 0);
    }
}
