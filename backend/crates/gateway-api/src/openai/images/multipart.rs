//! Normalize file uploads into the existing JSON image-edit contract.

use std::collections::BTreeMap;

use axum::{
    body::{Body, Bytes},
    http::StatusCode,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use multer::{Constraints, Multipart, SizeLimit};
use serde_json::{Map, Value, json};

use crate::openai::responses::{ProtocolError, ProtocolErrorBody};

const MAX_FORM_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PART_BYTES: u64 = 50 * 1024 * 1024;
const MAX_IMAGES: usize = 16;

pub(super) struct FormError {
    pub(super) status: StatusCode,
    pub(super) body: ProtocolErrorBody,
}

impl FormError {
    fn invalid(param: &'static str, message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            body: ProtocolErrorBody {
                error: ProtocolError {
                    kind: "invalid_request_error",
                    code: "invalid_image_form",
                    message: message.to_owned(),
                    param: Some(param.to_owned()),
                },
            },
        }
    }

    fn parser(error: &multer::Error) -> Self {
        if matches!(
            error,
            multer::Error::FieldSizeExceeded { .. } | multer::Error::StreamSizeExceeded { .. }
        ) {
            Self {
                status: StatusCode::PAYLOAD_TOO_LARGE,
                body: ProtocolErrorBody {
                    error: ProtocolError {
                        kind: "invalid_request_error",
                        code: "request_too_large",
                        message: "Image uploads allow 64 MiB per form and 50 MiB per part."
                            .to_owned(),
                        param: None,
                    },
                },
            }
        } else {
            Self::invalid("request", "The image multipart form is malformed.")
        }
    }
}

pub(super) async fn decode_edit_form(body: Body, content_type: &str) -> Result<Bytes, FormError> {
    let boundary =
        multer::parse_boundary(content_type).map_err(|error| FormError::parser(&error))?;
    let constraints = Constraints::new().size_limit(
        SizeLimit::new()
            .whole_stream(MAX_FORM_BYTES)
            .per_field(MAX_PART_BYTES),
    );
    let mut multipart = Multipart::with_constraints(body.into_data_stream(), boundary, constraints);
    let mut fields = Map::new();
    let mut images = Vec::new();
    let mut indexed_images = BTreeMap::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| FormError::parser(&error))?
    {
        let name = field
            .name()
            .ok_or_else(|| FormError::invalid("request", "Every form part needs a name."))?
            .to_owned();
        let content_type = field.content_type().map(ToString::to_string);
        let is_file = field.file_name().is_some();
        let data = field
            .bytes()
            .await
            .map_err(|error| FormError::parser(&error))?;
        if name == "mask" {
            if fields.contains_key("mask") {
                return Err(FormError::invalid("mask", "Only one mask is allowed."));
            }
            let url = image_data_url(&data, content_type.as_deref(), true)?;
            fields.insert("mask".to_owned(), json!({"image_url":url}));
        } else if name == "image" || name == "image[]" {
            if !indexed_images.is_empty() {
                return Err(FormError::invalid(
                    "image",
                    "Do not mix indexed and unindexed image fields.",
                ));
            }
            images.push(json!({
                "image_url":image_data_url(&data, content_type.as_deref(), false)?
            }));
        } else if let Some(index) = image_index(&name) {
            if !images.is_empty() {
                return Err(FormError::invalid(
                    "image",
                    "Do not mix indexed and unindexed image fields.",
                ));
            }
            let image = json!({
                "image_url":image_data_url(&data, content_type.as_deref(), false)?
            });
            if indexed_images.insert(index, image).is_some() {
                return Err(FormError::invalid("image", "Image indexes must be unique."));
            }
        } else {
            if is_file {
                return Err(FormError::invalid("request", "Unsupported file field."));
            }
            insert_text_field(&mut fields, &name, &data)?;
        }
        if images.len() + indexed_images.len() > MAX_IMAGES {
            return Err(FormError::invalid(
                "image",
                "At most 16 input images are supported.",
            ));
        }
    }
    images.extend(indexed_images.into_values());
    if images.is_empty() {
        return Err(FormError::invalid(
            "image",
            "At least one image file is required.",
        ));
    }
    for name in ["model", "prompt"] {
        if fields
            .get(name)
            .and_then(Value::as_str)
            .is_none_or(|value| value.trim().is_empty())
        {
            return Err(FormError::invalid(
                name,
                "Image editing requires a non-empty model and prompt.",
            ));
        }
    }
    fields.insert("images".to_owned(), Value::Array(images));
    serde_json::to_vec(&fields)
        .map(Bytes::from)
        .map_err(|_| FormError::invalid("request", "The image request cannot be encoded."))
}

fn image_index(name: &str) -> Option<u32> {
    let index = name.strip_prefix("image[")?.strip_suffix(']')?;
    if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    index.parse().ok()
}

fn image_data_url(data: &[u8], declared: Option<&str>, mask: bool) -> Result<String, FormError> {
    let mime = if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if data.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP".as_slice()) {
        "image/webp"
    } else {
        return Err(FormError::invalid(
            if mask { "mask" } else { "image" },
            "Image files must have a PNG, JPEG or WebP signature.",
        ));
    };
    if mask && mime != "image/png" {
        return Err(FormError::invalid("mask", "The image mask must be PNG."));
    }
    if let Some(declared) = declared
        && declared != mime
        && declared != "application/octet-stream"
        && !(declared == "image/jpg" && mime == "image/jpeg")
    {
        return Err(FormError::invalid(
            "image",
            "The image media type does not match its file signature.",
        ));
    }
    Ok(format!("data:{mime};base64,{}", STANDARD.encode(data)))
}

fn insert_text_field(
    fields: &mut Map<String, Value>,
    name: &str,
    data: &[u8],
) -> Result<(), FormError> {
    if fields.contains_key(name) {
        return Err(FormError::invalid(
            "request",
            "Duplicate form fields are not allowed.",
        ));
    }
    let text = std::str::from_utf8(data)
        .map_err(|_| FormError::invalid("request", "Text fields must be UTF-8."))?;
    let value = match name {
        "model" | "prompt" | "size" | "quality" | "response_format" | "background"
        | "output_format" | "input_fidelity" | "moderation" | "user" => {
            Value::String(text.to_owned())
        }
        "n" | "output_compression" => {
            let number = text.trim().parse::<u64>().map_err(|_| {
                FormError::invalid(
                    "request",
                    "Numeric form fields must be non-negative integers.",
                )
            })?;
            if (name == "n" && number == 0) || (name == "output_compression" && number > 100) {
                return Err(FormError::invalid(
                    "request",
                    "Numeric form value is out of range.",
                ));
            }
            Value::from(number)
        }
        "stream" if text.trim() == "false" => Value::Bool(false),
        "stream" => {
            return Err(FormError::invalid(
                "stream",
                "Image uploads currently support non-streaming responses only.",
            ));
        }
        _ => {
            return Err(FormError::invalid(
                "request",
                "This multipart field is not supported by the image-edit adapter.",
            ));
        }
    };
    fields.insert(name.to_owned(), value);
    Ok(())
}
