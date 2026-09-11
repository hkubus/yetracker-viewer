//! Library surface of the Rust API port. `main.rs` is a thin binary over
//! [`run`]; the modules mirror the TypeScript source layout one-for-one.

pub mod catalogs;
pub mod config;
pub mod cover_version;
pub mod db;
pub mod error;
pub mod media;
pub mod playable;
pub mod rank;
pub mod repair;
pub mod request;
pub mod serve;
pub mod state;
pub mod text;
