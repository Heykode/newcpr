//! Compress only bounded administrator JSON responses.

use axum::{
    body::HttpBody,
    http::{Response, header},
};
use tower_http::compression::predicate::Predicate;

#[derive(Clone, Copy)]
pub(super) struct AdminJson;

impl Predicate for AdminJson {
    fn should_compress<B: HttpBody>(&self, response: &Response<B>) -> bool {
        response
            .body()
            .size_hint()
            .exact()
            .is_some_and(|size| size >= 1024)
            && response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(';').next())
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
    }
}
