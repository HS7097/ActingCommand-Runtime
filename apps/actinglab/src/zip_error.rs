use super::CliError;
use std::io;

pub(super) fn zip_write_error(err: zip::result::ZipError) -> CliError {
    CliError::package_invalid(format!("zip write failed: {err}"))
}

pub(super) fn zip_io_error(err: io::Error) -> CliError {
    CliError::package_invalid(format!("zip write failed: {err}"))
}
