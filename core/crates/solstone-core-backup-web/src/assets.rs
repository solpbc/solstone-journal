use axum::{
    body::Body,
    http::{StatusCode, header},
    response::Response,
};

const WORKSPACE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../solstone/apps/backup/workspace.html"
));
const JS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../solstone/apps/backup/static/backup.js"
));
const CSS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../solstone/apps/backup/static/backup.css"
));
const NOT_FOUND: &str = "<!doctype html>\n<html lang=en>\n<title>404 Not Found</title>\n<h1>Not Found</h1>\n<p>The requested URL was not found on the server. If you entered the URL manually please check your spelling and try again.</p>\n";

fn bytes(status: StatusCode, bytes: &'static [u8], content_type: &'static str) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .expect("backup asset response")
}
pub async fn workspace() -> Response {
    bytes(StatusCode::OK, WORKSPACE, "text/html; charset=utf-8")
}
pub async fn background() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(NOT_FOUND))
        .expect("backup background response")
}
pub async fn static_asset(axum::extract::Path(name): axum::extract::Path<String>) -> Response {
    match name.as_str() {
        "backup.js" => bytes(StatusCode::OK, JS, "text/javascript; charset=utf-8"),
        "backup.css" => bytes(StatusCode::OK, CSS, "text/css; charset=utf-8"),
        _ => background().await,
    }
}
