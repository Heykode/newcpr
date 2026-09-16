use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nsynthetic parser fixture";

struct Part<'a> {
    name: &'a str,
    mime: Option<&'a str>,
    body: &'a [u8],
}

fn form(parts: &[Part<'_>]) -> Vec<u8> {
    let mut body = Vec::new();
    for part in parts {
        body.extend_from_slice(b"--cpr-test-boundary\r\nContent-Disposition: form-data; name=\"");
        body.extend_from_slice(part.name.as_bytes());
        body.extend_from_slice(b"\"");
        if part.mime.is_some() {
            body.extend_from_slice(b"; filename=\"upload.png\"");
        }
        body.extend_from_slice(b"\r\n");
        if let Some(mime) = part.mime {
            body.extend_from_slice(format!("Content-Type: {mime}\r\n").as_bytes());
        }
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(part.body);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(b"--cpr-test-boundary--\r\n");
    body
}

fn fields<'a>(images: &'a [Part<'a>]) -> Vec<Part<'a>> {
    let mut parts = vec![
        Part {
            name: "model",
            mime: None,
            body: b"gpt-image-2",
        },
        Part {
            name: "prompt",
            mime: None,
            body: b"Edit the image",
        },
    ];
    parts.extend(images.iter().map(|part| Part {
        name: part.name,
        mime: part.mime,
        body: part.body,
    }));
    parts
}

async fn send(
    execution: Arc<ImageExecution>,
    body: Vec<u8>,
    content_type: &str,
) -> axum::response::Response {
    api_router(execution)
        .await
        .oneshot(
            Request::post("/v1/images/edits")
                .header(AUTHORIZATION, "Bearer sk_images_test")
                .header("content-type", content_type)
                .header("session-id", "image-session")
                .header("x-codex-image-turn-id", "image-turn")
                .body(Body::from(body))
                .expect("upload"),
        )
        .await
        .expect("handler")
}

#[tokio::test]
async fn multipart_normalizes_images_mask_and_numeric_fields_into_existing_json_path() {
    let execution = ImageExecution::new();
    let parts = [
        Part {
            name: "image[]",
            mime: Some("image/png"),
            body: PNG,
        },
        Part {
            name: "image[]",
            mime: Some("application/octet-stream"),
            body: PNG,
        },
        Part {
            name: "mask",
            mime: Some("image/png"),
            body: PNG,
        },
        Part {
            name: "n",
            mime: None,
            body: b"1",
        },
        Part {
            name: "output_compression",
            mime: None,
            body: b"70",
        },
        Part {
            name: "stream",
            mime: None,
            body: b"false",
        },
    ];
    let response = send(
        Arc::clone(&execution),
        form(&fields(&parts)),
        "multipart/form-data; boundary=\"cpr-test-boundary\"",
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body"),
        IMAGE_RESPONSE
    );
    let captured = execution.captured();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].endpoint, "/v1/images/edits");
    assert_eq!(captured[0].context["session_id"], "image-session");
    assert_eq!(captured[0].context["image_turn_id"], "image-turn");
    let body: Value = serde_json::from_slice(&captured[0].body).expect("normalized JSON");
    assert_eq!(body["images"].as_array().expect("images").len(), 2);
    let expected = format!("data:image/png;base64,{}", STANDARD.encode(PNG));
    assert_eq!(body["images"][0]["image_url"], expected);
    assert_eq!(body["mask"]["image_url"], expected);
    assert_eq!(body["n"], 1);
    assert_eq!(body["output_compression"], 70);
    assert_eq!(body["stream"], false);
    assert_eq!(execution.committed_statuses(), [201]);
}

#[tokio::test]
async fn multipart_supports_newapi_single_and_indexed_image_fields() {
    for name in ["image", "image[]", "image[0]"] {
        let execution = ImageExecution::new();
        let parts = [Part {
            name,
            mime: Some("image/png"),
            body: PNG,
        }];
        let response = send(
            Arc::clone(&execution),
            form(&fields(&parts)),
            "multipart/form-data; boundary=cpr-test-boundary",
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED, "{name}");
        let captured = execution.captured();
        let body: Value = serde_json::from_slice(&captured[0].body).expect("JSON");
        assert_eq!(body["images"].as_array().expect("images").len(), 1);
    }
}

#[tokio::test]
async fn malformed_or_unsupported_uploads_fail_before_selecting_an_account() {
    let execution = ImageExecution::new();
    let invalid = [
        vec![],
        vec![Part {
            name: "image",
            mime: Some("image/png"),
            body: b"",
        }],
        vec![Part {
            name: "image",
            mime: Some("image/jpeg"),
            body: PNG,
        }],
        vec![
            Part {
                name: "image",
                mime: Some("image/png"),
                body: PNG,
            },
            Part {
                name: "stream",
                mime: None,
                body: b"true",
            },
        ],
        vec![
            Part {
                name: "image",
                mime: Some("image/png"),
                body: PNG,
            },
            Part {
                name: "n",
                mime: None,
                body: b"0",
            },
        ],
        vec![
            Part {
                name: "image",
                mime: Some("image/png"),
                body: PNG,
            },
            Part {
                name: "unknown_option",
                mime: None,
                body: b"do not drop",
            },
        ],
        vec![
            Part {
                name: "image[0]",
                mime: Some("image/png"),
                body: PNG,
            },
            Part {
                name: "image[0]",
                mime: Some("image/png"),
                body: PNG,
            },
        ],
        vec![
            Part {
                name: "image",
                mime: Some("image/png"),
                body: PNG,
            },
            Part {
                name: "image[0]",
                mime: Some("image/png"),
                body: PNG,
            },
        ],
    ];
    for parts in invalid {
        let response = send(
            Arc::clone(&execution),
            form(&fields(&parts)),
            "multipart/form-data; boundary=cpr-test-boundary",
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let response = send(
        Arc::clone(&execution),
        b"malformed".to_vec(),
        "multipart/form-data; boundary=missing",
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(execution.captured().is_empty());
}

#[tokio::test]
async fn upload_authentication_happens_before_reading_the_body() {
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        api_router(ImageExecution::new()).await.oneshot(
            Request::post("/v1/images/edits")
                .header(
                    "content-type",
                    "multipart/form-data; boundary=cpr-test-boundary",
                )
                .body(Body::from_stream(futures::stream::once(
                    futures::future::pending::<Result<Bytes, std::io::Error>>(),
                )))
                .expect("unauthenticated request"),
        ),
    )
    .await
    .expect("auth must not await body")
    .expect("handler");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn oversized_upload_part_is_rejected_before_execution() {
    let execution = ImageExecution::new();
    let oversized = vec![b'x'; 50 * 1024 * 1024 + 1];
    let parts = [Part {
        name: "image",
        mime: Some("image/png"),
        body: &oversized,
    }];
    let response = send(
        Arc::clone(&execution),
        form(&fields(&parts)),
        "multipart/form-data; boundary=cpr-test-boundary",
    )
    .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(execution.captured().is_empty());
}

#[tokio::test]
async fn compressed_multipart_fails_before_consuming_the_upload() {
    let execution = ImageExecution::new();
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        api_router(execution.clone()).await.oneshot(
            Request::post("/v1/images/edits")
                .header(AUTHORIZATION, "Bearer sk_images_test")
                .header(
                    "content-type",
                    "multipart/form-data; boundary=cpr-test-boundary",
                )
                .header("content-encoding", "gzip")
                .body(Body::from_stream(futures::stream::once(
                    futures::future::pending::<Result<Bytes, std::io::Error>>(),
                )))
                .expect("compressed upload"),
        ),
    )
    .await
    .expect("reject before body")
    .expect("handler");
    assert!(response.status().is_client_error());
    assert!(execution.captured().is_empty());
}

#[tokio::test]
async fn indexed_images_are_sorted_and_duplicate_masks_are_rejected() {
    let execution = ImageExecution::new();
    let second = b"\x89PNG\r\n\x1a\nsecond synthetic fixture";
    let parts = [
        Part {
            name: "image[2]",
            mime: Some("image/png"),
            body: second,
        },
        Part {
            name: "image[0]",
            mime: Some("image/png"),
            body: PNG,
        },
    ];
    let response = send(
        Arc::clone(&execution),
        form(&fields(&parts)),
        "multipart/form-data; boundary=cpr-test-boundary",
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let captured = execution.captured();
    let body: Value = serde_json::from_slice(&captured[0].body).expect("JSON");
    assert_eq!(
        body["images"][0]["image_url"],
        format!("data:image/png;base64,{}", STANDARD.encode(PNG))
    );
    assert_eq!(
        body["images"][1]["image_url"],
        format!("data:image/png;base64,{}", STANDARD.encode(second))
    );
    let parts = [
        Part {
            name: "image",
            mime: Some("image/png"),
            body: PNG,
        },
        Part {
            name: "mask",
            mime: Some("image/png"),
            body: PNG,
        },
        Part {
            name: "mask",
            mime: Some("image/png"),
            body: PNG,
        },
    ];
    let response = send(
        Arc::clone(&execution),
        form(&fields(&parts)),
        "multipart/form-data; boundary=cpr-test-boundary",
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(execution.captured().len(), 1);
}
