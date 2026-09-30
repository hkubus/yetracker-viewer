//! Library surface of the API. `main.rs` is a thin binary: it loads the
//! [`config`], opens the [`db`], starts the background [`sync`] and runs
//! [`http::serve`] over the [`routes`].

pub mod backfill;
pub mod catalogs;
pub mod config;
pub mod cover_version;
pub mod db;
pub mod dominant_color;
pub mod downloader;
pub mod error;
pub mod http;
pub mod importer;
pub mod media;
pub mod playable;
pub mod public_net;
pub mod rank;
pub mod repair;
pub mod request;
pub mod routes;
pub mod search_text;
pub mod serve;
pub mod state;
pub mod sync;
pub mod text;
