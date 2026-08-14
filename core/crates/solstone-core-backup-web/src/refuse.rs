use axum::{http::StatusCode, response::Response};

use crate::response;

pub const BACKUP_ENABLE_NOT_IMPLEMENTED_NATIVE: &str = "backup_enable_not_implemented_native";
pub const BACKUP_ENABLE_HOSTED_NOT_IMPLEMENTED_NATIVE: &str =
    "backup_enable_hosted_not_implemented_native";
pub const BACKUP_DESTINATION_NOT_IMPLEMENTED_NATIVE: &str =
    "backup_destination_not_implemented_native";
pub const BACKUP_RECOVERY_KEY_ROTATE_NOT_IMPLEMENTED_NATIVE: &str =
    "backup_recovery_key_rotate_not_implemented_native";
pub const BACKUP_TEARDOWN_NOT_IMPLEMENTED_NATIVE: &str = "backup_teardown_not_implemented_native";
pub const BACKUP_RESTORE_NOT_IMPLEMENTED_NATIVE: &str = "backup_restore_not_implemented_native";
pub const BACKUP_RESTORE_HOSTED_NOT_IMPLEMENTED_NATIVE: &str =
    "backup_restore_hosted_not_implemented_native";
pub const BACKUP_OFFLOAD_RESTORE_NOT_IMPLEMENTED_NATIVE: &str =
    "backup_offload_restore_not_implemented_native";

pub fn native_refusal(code: &str) -> Response {
    response::error(
        StatusCode::NOT_IMPLEMENTED,
        "I couldn't complete this backup operation in the native Convey surface yet.",
        code,
        "",
    )
}
