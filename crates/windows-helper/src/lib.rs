//! Couchlink Windows helper — protocol + op dispatch (service binary is Windows-only).

pub mod ops;
pub mod protocol;

#[cfg(windows)]
pub mod pipe_server;

#[cfg(windows)]
pub mod service;
