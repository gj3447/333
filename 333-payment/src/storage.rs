use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use bincode::Options;
use fs2::FileExt;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::PaymentError;

const MAGIC: &[u8; 16] = b"PAYMENT333STATE1";

#[derive(Debug)]
pub(crate) enum Persistence {
    Memory,
    File(FileStore),
}

impl Persistence {
    pub fn memory() -> Self {
        Self::Memory
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, PaymentError> {
        Ok(Self::File(FileStore::open(path.as_ref())?))
    }

    pub fn exists(&self) -> bool {
        match self {
            Self::Memory => false,
            Self::File(store) => store.path.exists(),
        }
    }

    pub fn load<T: DeserializeOwned>(&self) -> Result<T, PaymentError> {
        match self {
            Self::Memory => Err(PaymentError::Storage("memory store has no snapshot".into())),
            Self::File(store) => store.load(),
        }
    }

    pub fn persist<T: Serialize>(&self, value: &T) -> Result<(), PaymentError> {
        match self {
            Self::Memory => Ok(()),
            Self::File(store) => store.persist(value),
        }
    }
}

pub(crate) struct FileStore {
    path: PathBuf,
    _lock: File,
}

impl std::fmt::Debug for FileStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileStore")
            .field("path", &self.path)
            .finish()
    }
}

impl FileStore {
    fn open(path: &Path) -> Result<Self, PaymentError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(storage)?;
        }
        let lock_path = path.with_extension("lock");
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(storage)?;
        lock.try_lock_exclusive()
            .map_err(|_| PaymentError::StoreBusy)?;
        Ok(Self {
            path: path.to_path_buf(),
            _lock: lock,
        })
    }

    fn load<T: DeserializeOwned>(&self) -> Result<T, PaymentError> {
        let mut file = File::open(&self.path).map_err(storage)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(storage)?;
        if bytes.len() < MAGIC.len() + 8 + 32 || &bytes[..MAGIC.len()] != MAGIC {
            return Err(PaymentError::CorruptStore);
        }
        let len_start = MAGIC.len();
        let mut len_bytes = [0u8; 8];
        len_bytes.copy_from_slice(&bytes[len_start..len_start + 8]);
        let payload_len = u64::from_le_bytes(len_bytes) as usize;
        let payload_start = len_start + 8;
        let checksum_start = payload_start
            .checked_add(payload_len)
            .ok_or(PaymentError::CorruptStore)?;
        if checksum_start + 32 != bytes.len() {
            return Err(PaymentError::CorruptStore);
        }
        let payload = &bytes[payload_start..checksum_start];
        let expected: [u8; 32] = Sha256::digest(payload).into();
        if bytes[checksum_start..] != expected {
            return Err(PaymentError::CorruptStore);
        }
        codec()
            .deserialize(payload)
            .map_err(|_| PaymentError::CorruptStore)
    }

    fn persist<T: Serialize>(&self, value: &T) -> Result<(), PaymentError> {
        let payload = codec().serialize(value).map_err(storage)?;
        let checksum: [u8; 32] = Sha256::digest(&payload).into();
        let tmp = self
            .path
            .with_extension(format!("tmp-{}", std::process::id()));
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp)
            .map_err(storage)?;
        file.write_all(MAGIC).map_err(storage)?;
        file.write_all(&(payload.len() as u64).to_le_bytes())
            .map_err(storage)?;
        file.write_all(&payload).map_err(storage)?;
        file.write_all(&checksum).map_err(storage)?;
        file.sync_all().map_err(storage)?;
        fs::rename(&tmp, &self.path).map_err(storage)?;
        if let Some(parent) = self.path.parent() {
            File::open(parent)
                .and_then(|f| f.sync_all())
                .map_err(storage)?;
        }
        Ok(())
    }
}

fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .reject_trailing_bytes()
}

fn storage(error: impl std::fmt::Display) -> PaymentError {
    PaymentError::Storage(error.to_string())
}
