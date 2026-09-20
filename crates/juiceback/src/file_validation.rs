pub use juiceutils::file_validation::*;

use crate::error::AppError;

/// Map a `FileValidation` to its `BlockedFileType` error.
///
/// Returns `None` for `Allowed` so call sites collapse their repeated
/// four-arm match into a single `if let Some(err)` check.
#[must_use]
pub fn blocked_file_error(validation: FileValidation) -> Option<AppError> {
    match validation {
        FileValidation::Allowed => None,
        FileValidation::BlockedExtension { tier, .. }
        | FileValidation::BlockedMagic { tier, .. } => {
            Some(AppError::BlockedFileType(friendly_block_reason(tier)))
        }
        FileValidation::Empty => Some(AppError::BlockedFileType(
            "Empty files are not allowed".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_maps_to_none() {
        assert!(blocked_file_error(FileValidation::Allowed).is_none());
    }

    #[test]
    fn empty_maps_to_blocked() {
        let err = blocked_file_error(FileValidation::Empty);
        assert!(matches!(err, Some(AppError::BlockedFileType(_))));
    }

    #[test]
    fn blocked_variants_map_to_blocked() {
        for validation in [
            validate_filename("app.exe", ProtectionLevel::Low),
            validate_file("renamed.txt", b"MZ\x90\x00\x03", ProtectionLevel::Low),
        ] {
            assert!(matches!(
                blocked_file_error(validation),
                Some(AppError::BlockedFileType(_))
            ));
        }
    }
}
