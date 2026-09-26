use std::sync::Arc;

use axum::routing;

use crate::state::AppState;

pub mod completion;
pub mod create;
pub mod metadata;
pub mod parallel;
pub mod patch;
pub mod query;
pub mod state;
pub mod types;

#[cfg(test)]
mod tests;

pub use create::create_upload_handler;
pub use patch::patch_upload_handler;
pub use query::{delete_upload_handler, get_upload_handler, options_handler};

#[must_use]
pub fn tus_routes() -> axum::Router<Arc<AppState>> {
    axum::Router::new()
        .route(
            "/api/tus",
            routing::post(create_upload_handler).options(options_handler),
        )
        .route(
            "/api/tus/{id}",
            routing::get(get_upload_handler)
                .patch(patch_upload_handler)
                .delete(delete_upload_handler),
        )
}
