use axum::{body::Body, http::{header, StatusCode, Uri}, response::Response};
use include_dir::{include_dir, Dir};

static DIST: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/web/dist");

pub async fn serve(uri: Uri) -> Result<Response<Body>, StatusCode> {
    let path = uri.path().trim_start_matches('/');
    let candidate = if path.is_empty() { "index.html" } else { path };
    let file = DIST.get_file(candidate).or_else(|| DIST.get_file("index.html"))
        .ok_or(StatusCode::NOT_FOUND)?;
    let mime = mime_guess::from_path(candidate).first_or_octet_stream();
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, mime.as_ref())
        .body(Body::from(file.contents()))
        .unwrap())
}
