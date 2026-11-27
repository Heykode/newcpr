use std::io::{Read, Write, copy, repeat};

use axum::http::{HeaderMap, HeaderValue, header::CONTENT_ENCODING};
use gateway_api::openai::responses::{RequestDecodeError, decode_request_with_headers};
use serde_json::json;

use super::{openai_protocol_context, openai_wire_body};

const ENCODINGS: [&str; 3] = ["gzip", "deflate", "zstd"];
const REQUEST: &[u8] = br#"{"model":"gpt-test","input":"hello","future_field":{"value":1}}"#;
const DECOMPRESSED_LIMIT: u64 = 64 * 1024 * 1024;

#[test]
fn legacy_turn_header_is_opaque_without_replacing_body_or_standard_metadata() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use gateway_protocol::openai::codex::codex_turn_id;

    for (extra, metadata, expected) in [
        (json!({}), None, None),
        (json!({"turn_id":"body-turn"}), None, Some("body-turn")),
        (
            json!({"client_metadata":{"turn_id":"client-turn"}}),
            None,
            Some("client-turn"),
        ),
        (
            json!({}),
            Some(r#"{"turn_id":"standard-turn"}"#),
            Some("standard-turn"),
        ),
    ] {
        let mut request_headers = HeaderMap::new();
        request_headers.insert("x-codex-turn-id", HeaderValue::from_static("legacy-turn"));
        if let Some(metadata) = metadata {
            request_headers.insert("x-codex-turn-metadata", HeaderValue::from_static(metadata));
        }
        let mut body = json!({"model":"gpt-test","input":"hello"});
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let decoded =
            decode_request_with_headers(body.to_string().as_bytes(), &request_headers).unwrap();
        let context = openai_protocol_context(&decoded);
        assert!(!context.contains_key("turn_id"));
        assert_eq!(
            codex_turn_id(openai_wire_body(&decoded), context).as_deref(),
            expected
        );
        let opaque = context["opaque_request_headers"].as_array().unwrap();
        assert!(opaque.contains(&json!(["x-codex-turn-id", STANDARD.encode(b"legacy-turn")])));
        assert_eq!(openai_wire_body(&decoded), body.as_object().unwrap());
    }
}

pub(super) fn encode_body(encoding: &str, mut data: impl Read) -> Vec<u8> {
    match encoding {
        "gzip" => {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            copy(&mut data, &mut encoder).expect("gzip encode");
            encoder.finish().expect("gzip finish")
        }
        "deflate" => {
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
            copy(&mut data, &mut encoder).expect("deflate encode");
            encoder.finish().expect("deflate finish")
        }
        "zstd" => zstd::stream::encode_all(data, 3).expect("zstd encode"),
        _ => panic!("unsupported fixture encoding"),
    }
}

fn headers(encoding: &str) -> HeaderMap {
    HeaderMap::from_iter([(
        CONTENT_ENCODING,
        HeaderValue::from_str(encoding).expect("encoding header"),
    )])
}

#[test]
fn http_decode_should_preserve_compressed_request_fields() {
    let expected = serde_json::from_slice::<serde_json::Value>(REQUEST).expect("fixture JSON");
    for encoding in ENCODINGS {
        let compressed = encode_body(encoding, REQUEST);
        let decoded = decode_request_with_headers(&compressed, &headers(encoding)).expect(encoding);
        assert_eq!(
            openai_wire_body(&decoded),
            expected.as_object().expect("object")
        );
    }
}

#[test]
fn http_decode_should_accept_plain_identity_and_case_insensitive_encodings() {
    for request_headers in [HeaderMap::new(), headers(""), headers(" identity ")] {
        assert!(decode_request_with_headers(REQUEST, &request_headers).is_ok());
    }
    let compressed = encode_body("gzip", REQUEST);
    assert!(decode_request_with_headers(&compressed, &headers(" GZip ")).is_ok());
}

#[test]
fn http_decode_should_reject_unknown_and_stacked_encodings() {
    let mut repeated = headers("identity");
    repeated.append(CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    for request_headers in [headers("br"), headers("gzip, zstd"), repeated] {
        let error = decode_request_with_headers(REQUEST, &request_headers)
            .expect_err("unsupported encoding");
        assert!(matches!(
            error,
            RequestDecodeError::UnsupportedContentEncoding { .. }
        ));
        assert_eq!(
            error.protocol_body().into_value()["error"]["code"],
            "unsupported_content_encoding"
        );
    }
}

#[test]
fn http_decode_should_reject_corrupted_and_truncated_compressed_bodies() {
    for encoding in ENCODINGS {
        let mut truncated = encode_body(encoding, REQUEST);
        truncated.truncate(truncated.len() - 4);
        for body in [b"not-a-compressed-stream".as_slice(), truncated.as_slice()] {
            let error = decode_request_with_headers(body, &headers(encoding)).expect_err(encoding);
            assert_eq!(
                error.protocol_body().into_value(),
                json!({"error": {
                    "type": "invalid_request_error", "code": "invalid_json",
                    "message": "Request body must be valid JSON."
                }})
            );
        }
    }
}

#[test]
fn http_decode_should_read_all_gzip_members_and_zstd_frames() {
    for encoding in ["gzip", "zstd"] {
        let mut compressed = encode_body(encoding, &REQUEST[..12]);
        compressed.extend(encode_body(encoding, &REQUEST[12..]));
        let decoded = decode_request_with_headers(&compressed, &headers(encoding)).expect(encoding);
        assert_eq!(openai_wire_body(&decoded)["input"], "hello");
    }
}

#[test]
fn http_decode_should_not_ignore_invalid_trailing_members() {
    for encoding in ["gzip", "zstd"] {
        let mut compressed = encode_body(encoding, REQUEST);
        compressed.extend(encode_body(encoding, b"invalid trailing JSON".as_slice()));
        assert_eq!(
            decode_request_with_headers(&compressed, &headers(encoding)).expect_err(encoding),
            RequestDecodeError::MalformedJson
        );
    }
}

#[test]
fn http_decode_should_reject_zstd_frames_with_an_excessive_window() {
    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 3).expect("zstd encoder");
    encoder.window_log(27).expect("128 MiB window");
    encoder.write_all(REQUEST).expect("write JSON");
    // 提前输出帧头，避免编码器根据最终小正文缩减窗口。
    encoder.flush().expect("flush frame header");
    let compressed = encoder.finish().expect("finish frame");
    assert_eq!(
        zstd::stream::decode_all(compressed.as_slice()).expect("valid zstd frame"),
        REQUEST
    );
    assert_eq!(
        decode_request_with_headers(&compressed, &headers("zstd")).expect_err("oversized window"),
        RequestDecodeError::MalformedJson
    );
}

#[test]
fn http_decode_should_accept_the_exact_decompressed_limit() {
    let padding = repeat(b' ').take(DECOMPRESSED_LIMIT - REQUEST.len() as u64);
    let compressed = encode_body("zstd", REQUEST.chain(padding));
    let decoded = decode_request_with_headers(&compressed, &headers("zstd")).expect("exact limit");
    assert_eq!(decoded.metadata().requested_model(), "gpt-test");
}

#[test]
fn http_decode_should_reject_bodies_above_the_decompressed_limit() {
    for encoding in ENCODINGS {
        let compressed = encode_body(encoding, repeat(0).take(DECOMPRESSED_LIMIT + 1));
        let error =
            decode_request_with_headers(&compressed, &headers(encoding)).expect_err(encoding);
        assert_eq!(
            error,
            RequestDecodeError::DecompressedBodyTooLarge,
            "{encoding}"
        );
    }
}

#[test]
fn http_decode_should_stop_at_limit_before_reading_invalid_trailing_frames() {
    for encoding in ["zstd", "gzip"] {
        // 尾部故意损坏：若先完整解压再检查长度，就会错误地返回 invalid_json，
        // 这个断言验证越界时已经停止读取，而不只是最终返回了某个超限错误。
        let mut compressed = encode_body(encoding, repeat(0).take(DECOMPRESSED_LIMIT + 1));
        compressed.extend_from_slice(b"invalid trailing frame");
        let error =
            decode_request_with_headers(&compressed, &headers(encoding)).expect_err(encoding);
        assert_eq!(
            error,
            RequestDecodeError::DecompressedBodyTooLarge,
            "{encoding}"
        );
    }
}
