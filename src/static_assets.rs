use axum::{
    body::Body,
    http::{header, StatusCode, Uri},
    response::Response,
};
use include_dir::{include_dir, Dir};

static DIST: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/web/dist");

pub async fn serve(uri: Uri) -> Result<Response<Body>, StatusCode> {
    let path = uri.path().trim_start_matches('/');
    let candidate = if path.is_empty() { "index.html" } else { path };

    // SPA-style fallback: requested file > index.html. Use the *served* path for
    // mime detection so the SPA route fallback is delivered as text/html (not
    // application/octet-stream, which makes browsers download it as a file named
    // after the URL segment, e.g. visiting /runs/1 saved a file named "1").
    let (file, served_path) = match DIST.get_file(candidate) {
        Some(f) => (f, candidate),
        None => (
            DIST.get_file("index.html").ok_or(StatusCode::NOT_FOUND)?,
            "index.html",
        ),
    };

    let mime = mime_guess::from_path(served_path).first_or_octet_stream();
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, mime.as_ref())
        .body(Body::from(file.contents()))
        .unwrap())
}
