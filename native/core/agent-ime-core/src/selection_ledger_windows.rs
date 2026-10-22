//! Windows version of the selection claim ledger (same contract as the Unix one: a
//! claim is durable before librime may learn, and a replayed delivery ID is refused
//! while it remains in the bounded ledger). The per-user profile ACL keeps the files
//! private; an exclusive file lock serialises processes.

use crate::secure_directory::open_or_create_owned_leaf_directory;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const DIRECTORY_NAME: &str = ".agent-ime-selection-ledger-v1";
const LOCK_FILE_NAME: &str = "ledger.lock";
const LEDGER_FILE_NAME: &str = "delivery-ids.bin";
const MAGIC: &[u8; 8] = b"AIMSDL1\n";
const MAXIMUM_IDENTIFIERS: usize = 4_096;
const HEADER_BYTES: usize = MAGIC.len() + std::mem::size_of::<u32>();
const MAXIMUM_FILE_BYTES: usize = HEADER_BYTES + MAXIMUM_IDENTIFIERS * std::mem::size_of::<u64>();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SelectionClaim {
    Fresh,
    AlreadyClaimed,
    Unavailable,
}

pub(crate) struct SelectionDeliveryLedger {
    directory: PathBuf,
}

impl SelectionDeliveryLedger {
    pub(crate) fn new(user_directory: &Path) -> Self {
        Self {
            directory: user_directory.join(DIRECTORY_NAME),
        }
    }

    pub(crate) fn claim(&self, identifier: u64) -> SelectionClaim {
        if identifier == 0 {
            return SelectionClaim::Unavailable;
        }
        self.claim_locked(identifier)
            .unwrap_or(SelectionClaim::Unavailable)
    }

    fn claim_locked(&self, identifier: u64) -> io::Result<SelectionClaim> {
        open_or_create_owned_leaf_directory(&self.directory, 0o700)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.directory.join(LOCK_FILE_NAME))?;
        lock.try_lock().map_err(|_| io::Error::new(io::ErrorKind::WouldBlock, "ledger is busy"))?;
        let path = self.directory.join(LEDGER_FILE_NAME);
        let created = !path.exists();
        let mut ledger = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        let mut identifiers = read_identifiers(&mut ledger, created)?;
        if identifiers.contains(&identifier) {
            return Ok(SelectionClaim::AlreadyClaimed);
        }
        if identifiers.len() == MAXIMUM_IDENTIFIERS {
            identifiers.remove(0);
        }
        identifiers.push(identifier);
        let data = encode(&identifiers)?;
        ledger.seek(SeekFrom::Start(0))?;
        ledger.set_len(0)?;
        ledger.write_all(&data)?;
        ledger.sync_all()?;
        drop(lock);
        Ok(SelectionClaim::Fresh)
    }
}

fn read_identifiers(file: &mut File, created: bool) -> io::Result<Vec<u64>> {
    let length = file.metadata()?.len();
    if length > MAXIMUM_FILE_BYTES as u64 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "ledger exceeds its bound"));
    }
    if created || length == 0 {
        return Ok(Vec::new());
    }
    file.seek(SeekFrom::Start(0))?;
    let mut data = Vec::with_capacity(length as usize);
    Read::by_ref(file)
        .take((MAXIMUM_FILE_BYTES + 1) as u64)
        .read_to_end(&mut data)?;
    decode(&data)
}

fn encode(identifiers: &[u64]) -> io::Result<Vec<u8>> {
    if identifiers.len() > MAXIMUM_IDENTIFIERS
        || identifiers.contains(&0)
        || identifiers.iter().copied().collect::<HashSet<_>>().len() != identifiers.len()
    {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid delivery ID set"));
    }
    let mut data = Vec::with_capacity(HEADER_BYTES + identifiers.len() * 8);
    data.extend_from_slice(MAGIC);
    data.extend_from_slice(&(identifiers.len() as u32).to_be_bytes());
    for identifier in identifiers {
        data.extend_from_slice(&identifier.to_be_bytes());
    }
    Ok(data)
}

fn decode(data: &[u8]) -> io::Result<Vec<u64>> {
    if data.len() < HEADER_BYTES || data.len() > MAXIMUM_FILE_BYTES || &data[..MAGIC.len()] != MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid ledger header"));
    }
    let count = u32::from_be_bytes(data[MAGIC.len()..HEADER_BYTES].try_into().unwrap()) as usize;
    if count > MAXIMUM_IDENTIFIERS || data.len() != HEADER_BYTES + count * 8 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid ledger length"));
    }
    let mut identifiers = Vec::with_capacity(count);
    let mut unique = HashSet::with_capacity(count);
    for chunk in data[HEADER_BYTES..].chunks_exact(8) {
        let identifier = u64::from_be_bytes(chunk.try_into().unwrap());
        if identifier == 0 || !unique.insert(identifier) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid ledger entry"));
        }
        identifiers.push(identifier);
    }
    Ok(identifiers)
}
