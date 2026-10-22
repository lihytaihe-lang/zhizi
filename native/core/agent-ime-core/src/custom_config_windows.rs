//! Windows store for the managed custom config. The per-user profile ACL keeps the
//! directory private; replacement writes a temporary file and renames it over the
//! config (MoveFileEx with replace), and every change checks the expected contents.

use super::*;
use crate::secure_directory::open_or_create_owned_leaf_directory;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

pub(crate) struct CustomConfigStore {
    directory: PathBuf,
}

impl CustomConfigStore {
    pub(crate) fn open_or_create(path: &Path) -> io::Result<Self> {
        open_or_create_owned_leaf_directory(path, 0o700)?;
        Ok(Self {
            directory: path.to_path_buf(),
        })
    }

    pub(crate) fn read(&self) -> io::Result<Option<Vec<u8>>> {
        let path = self.directory.join(CONFIG_FILE_NAME);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.file_type().is_file() => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "custom config is not a regular file",
            )),
            Ok(metadata) if metadata.len() > MAXIMUM_CONFIG_BYTES as u64 => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "custom config exceeds its bound",
            )),
            Ok(_) => fs::read(&path).map(Some),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn read_managed(&self) -> io::Result<Option<Vec<u8>>> {
        let contents = self.read()?;
        if contents
            .as_deref()
            .is_some_and(|value| !is_managed_fuzzy_config(value))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "custom config is not managed by Agent Input Method",
            ));
        }
        Ok(contents)
    }

    pub(crate) fn replace(
        &self,
        contents: &[u8],
        expected_current: Option<&[u8]>,
    ) -> io::Result<()> {
        if contents.len() > MAXIMUM_CONFIG_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "custom config exceeds its bound",
            ));
        }
        if self.read()?.as_deref() != expected_current {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "custom config ownership changed",
            ));
        }
        for _ in 0..TEMPORARY_ATTEMPTS {
            let temporary = self.directory.join(format!(
                ".agent-pinyin-custom-{}-{}.tmp",
                std::process::id(),
                NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
            ));
            let mut file = match fs::OpenOptions::new().write(true).create_new(true).open(&temporary) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            };
            let result = file
                .write_all(contents)
                .and_then(|_| file.sync_all())
                .and_then(|_| {
                    drop(file);
                    fs::rename(&temporary, self.directory.join(CONFIG_FILE_NAME))
                });
            if result.is_err() {
                let _ = fs::remove_file(&temporary);
            }
            return result;
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "no exclusive custom config temporary name was available",
        ))
    }

    pub(crate) fn remove(&self, expected_current: &[u8]) -> io::Result<()> {
        let actual = self
            .read()?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "custom config is missing"))?;
        if actual != expected_current {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "custom config ownership changed",
            ));
        }
        fs::remove_file(self.directory.join(CONFIG_FILE_NAME))
    }
}
