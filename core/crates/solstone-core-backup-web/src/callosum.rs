use serde_json::json;
use solstone_core_callosum::CallosumOneShotSender;
use std::{path::Path, time::Duration};

pub fn request(root: &Path, command: &str) -> bool {
    let line = match serde_json::to_string(
        &json!({"tract":"supervisor","event":"request","cmd":["journal", "maintenance", "run", command]}),
    ) {
        Ok(value) => format!("{value}\n"),
        Err(_) => return false,
    };
    CallosumOneShotSender::new(root.join("health/callosum.sock"), Duration::from_secs(2))
        .send_line(&line)
        .is_ok()
}
