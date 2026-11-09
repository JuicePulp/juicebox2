/// Header carrying the shared service key between juiceback, juicehost, and the
/// router.
pub const HOST_API_KEY: &str = "x-juicehost-api-key";

/// Origin of the juiceback instance that made an internal request.
pub const BACKEND_ORIGIN: &str = "x-juiceback-origin";

/// Per-file capability that scopes a request to one file.
pub const FILE_CAPABILITY: &str = "x-juicehost-file-capability";

/// Overrides the MIME type guessed from the extension.
pub const MIME_TYPE: &str = "x-mime-type";

/// Declared total size for a live upload.
pub const LIVE_TOTAL: &str = "x-juicebox-live-total";

/// Delete token presented by the uploader.
pub const DELETE_TOKEN: &str = "x-delete-token";

/// Password supplied to open a protected file.
pub const FILE_PASSWORD: &str = "x-file-password";

/// Content encoding of an uploaded body (`gzip` only today).
pub const FILE_ENCODING: &str = "x-file-encoding";

/// Region selected for an upload.
pub const REGION: &str = "x-juicebox-region";

/// Marks a response as describing a still-uploading file.
pub const UPLOADING: &str = "x-juicebox-uploading";

/// Bytes received so far for a live upload.
pub const UPLOADED: &str = "x-juicebox-uploaded";

/// Total size of a live upload.
pub const TOTAL: &str = "x-juicebox-total";
