use std::sync::Arc;

use axum::{
    Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use gateway_core::provider_ports::TemporaryImageSource;

pub(crate) fn router(source: Arc<dyn TemporaryImageSource>) -> Router {
    Router::new()
        .route("/_cpr/excel-images/{capability}", get(download))
        .with_state(source)
}

async fn download(
    State(source): State<Arc<dyn TemporaryImageSource>>,
    Path(capability): Path<String>,
) -> Response {
    let Some(image) = source.read(&capability) else {
        return (StatusCode::NOT_FOUND, [(header::CACHE_CONTROL, "no-store")]).into_response();
    };
    (
        [
            (header::CONTENT_TYPE, image.content_type),
            (header::CACHE_CONTROL, "private, no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CONTENT_SECURITY_POLICY, "default-src 'none'"),
        ],
        image.bytes,
    )
        .into_response()
}
