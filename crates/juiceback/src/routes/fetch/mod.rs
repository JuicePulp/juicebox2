//! `JuiceBox` x Cobalt.Tools: paste a link (`YouTube`, tiktok, ...), juiceback
//! asks a self-hosted cobalt instance to process it, streams the result into
//! juicehost and hands the user a normal /f/ file like any other upload.

pub mod handlers;
pub mod job;
pub mod store;
pub mod types;
pub mod validation;
pub mod ytdlp;

pub use handlers::{fetch_services_handler, fetch_start_handler, fetch_status_handler};
pub use types::{
    FetchFileResponse, FetchServicesResponse, FetchStartRequest, FetchStartResponse,
    FetchStatusResponse,
};
pub use ytdlp::build_ytdlp_args_for_test;
