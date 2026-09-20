//! handles multipart file uploads. streams non-gzip data straight to juicehost.
//! gzip uploads get buffered, decompressed, then sent.

pub mod common;
pub mod direct;
pub mod internal;
pub mod multipart;
pub mod ultrafast;

pub use common::UploadResponse;
pub(crate) use common::{region_host, sanitize_filename};
pub use direct::{
    DirectUploadCompleteRequest, DirectUploadReserveRequest, DirectUploadReserveResponse,
    direct_upload_complete_handler, direct_upload_reserve_handler,
};
pub use internal::file_status_handler;
pub use multipart::{
    ReserveResponse, ReserveUploadRequest, reserve_upload_handler, upload_handler,
};
pub use ultrafast::{
    UltrafastCompleteRequest, UltrafastReserveRequest, UltrafastReserveResponse,
    device_upload_handler, ultrafast_complete_handler, ultrafast_reserve_handler,
};
