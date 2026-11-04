//! The `mirrorleaf-blob` protocol (plan §5.6): the screen shows a note's photos
//! from the blob store through it.
//!
//! A note stores a photo as `mirrorleaf-blob://<full>?thumb=<thumbnail>`. The
//! editor turns that into `<protocol base>/<full>/<thumbnail>`, which on macOS
//! and Linux is `mirrorleaf-blob://localhost/…` and on Android and Windows
//! `http://mirrorleaf-blob.localhost/…`. The answer is the full image if this
//! device holds it, else the thumbnail, in which case the full image is asked
//! for and the screen hears when it arrives.

use mirrorleaf_ffi::Engine;
use tauri::http::{Request, Response, StatusCode, header};
use tauri::{Runtime, UriSchemeContext, UriSchemeResponder};

pub const SCHEME: &str = "mirrorleaf-blob";

pub fn handle<R: Runtime>(
    _ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let path = request.uri().path().to_owned();
    tauri::async_runtime::spawn(async move {
        responder.respond(answer(&path).await);
    });
}

async fn answer(path: &str) -> Response<Vec<u8>> {
    let mut parts = path.trim_matches('/').split('/');
    let (Some(full), Some(thumbnail), None) = (parts.next(), parts.next(), parts.next()) else {
        return status(StatusCode::BAD_REQUEST);
    };
    let Some(engine) = Engine::get() else {
        return status(StatusCode::SERVICE_UNAVAILABLE);
    };
    match engine.photo(full.to_owned(), thumbnail.to_owned()).await {
        Ok(Some((bytes, is_full))) => Response::builder()
            .header(header::CONTENT_TYPE, "image/jpeg")
            // A full image never changes (it is named by its hash); a
            // thumbnail stands in until the full image arrives.
            .header(
                header::CACHE_CONTROL,
                if is_full {
                    "max-age=31536000, immutable"
                } else {
                    "no-store"
                },
            )
            .body(bytes)
            .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR)),
        Ok(None) => status(StatusCode::NOT_FOUND),
        Err(e) => {
            tracing::warn!("photo {path}: {e}");
            status(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

fn status(code: StatusCode) -> Response<Vec<u8>> {
    let mut response = Response::new(Vec::new());
    *response.status_mut() = code;
    response
}
