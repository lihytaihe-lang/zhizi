//! Durable claim ledger for librime learning; see the platform implementations.
#[cfg(unix)]
#[path = "selection_ledger_unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "selection_ledger_windows.rs"]
mod imp;

pub(crate) use imp::*;
