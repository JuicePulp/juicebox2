pub mod audio_embed;
pub mod bans;
pub mod common;
pub mod concat;
pub mod delete;
pub mod freeze;
pub mod general;
pub mod live;
pub mod preview;
pub mod zip_entry;
pub mod zip_list;
pub mod r2;
pub mod rename;
pub mod route;
pub mod serve;
pub mod shell;
pub mod stats;
pub mod store;

#[cfg(test)]
mod tests;

pub use audio_embed::audio_embed_media;
pub use bans::{BanHashRequest, BanSnapshotRequest, push_ban, push_snapshot, push_unban};
pub(crate) use common::deadline_body;
pub use concat::{ConcatRequest, concat_files};
pub use delete::delete_file;
pub use freeze::{freeze_file, unfreeze_file};
pub use general::{
    ciphertext_file, config_handler, favicon, health, index_handler, ip_handler, stat_file,
    storage_handler,
};
pub(crate) use general::serve_stored_bytes_inline;
pub use live::{
    live_part_status, register_live_upload, remove_live_upload, upload_progress,
};
pub use preview::preview_file_wildcard;
pub use r2::{demote_file, promote_file};
pub use rename::{RenameRequest, rename_file};
pub use serve::{serve_file_download, serve_file_wildcard};
pub use zip_entry::zip_entry_file;
pub(crate) use stats::viewer_ip_middleware;
pub use store::{store_file, store_file_streaming, store_file_ticket};
