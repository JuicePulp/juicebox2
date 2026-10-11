pub mod common;
pub mod concat;
pub mod delete;
pub mod freeze;
pub mod general;
pub mod preview;
pub mod rename;
pub mod serve;
pub mod shell;
pub mod stats;
pub mod store;

#[cfg(test)]
mod tests;

pub(crate) use common::deadline_body;
pub use concat::{ConcatRequest, concat_files};
pub use delete::delete_file;
pub use freeze::{freeze_file, unfreeze_file};
pub use general::{
    ciphertext_file, ciphertext_public, config_handler, health, index_handler, ip_handler,
    stat_file, storage_handler,
};
pub use preview::preview_file_wildcard;
pub use rename::{RenameRequest, rename_file};
pub use serve::{serve_file_download, serve_file_wildcard};
pub(crate) use stats::viewer_ip_middleware;
pub use store::{store_file, store_file_streaming, store_file_ticket};
