//! Library surface of the Rust API port. `main.rs` is a thin binary over
//! [`run`]; the modules mirror the TypeScript source layout one-for-one.

pub mod backfill;
pub mod catalogs;
pub mod config;
pub mod cover_version;
pub mod db;
pub mod dominant_color;
pub mod downloader;
pub mod error;
pub mod importer;
pub mod media;
pub mod playable;
pub mod rank;
pub mod repair;
pub mod request;
pub mod routes;
pub mod serve;
pub mod state;
pub mod text;
