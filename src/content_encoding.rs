//! HTTP-level zstd content coding, negotiated per request.
//!
//! Requests: a `Content-Encoding: zstd` body is decoded before any handler
//! runs, and the decoded size is held to the same limit as an identity body,
//! so a small compressed body cannot expand past what the service would have
//! accepted uncompressed. Any other non-identity coding is refused with 415.
//!
//! Responses: a JSON or text body is zstd-encoded when the request's
//! `Accept-Encoding` allows `zstd` and the body is large enough to gain.
//!
//! Every response carries `Accept-Encoding: zstd` (RFC 7694) so clients learn
//! that this server takes zstd request bodies and only then start sending them.
//!
//! The octet-stream protocol of `/cache_userdata` and batch `/recommend` is
//! already zstd at the application layer; clients do not encode it twice.

use std::io::{Cursor, Read};

use axum::body::{Body, Bytes, to_bytes};
use axum::extract::Request;
use axum::http::header::{
    ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderValue, VARY,
};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sonic_rs::json;

/// Limit for an identity body and for a decoded zstd body.
pub const MAX_BODY_BYTES: usize = 1000 * 1024 * 1024;

/// Responses smaller than this are sent as they are.
pub const MIN_COMPRESS_BYTES: usize = 1024;

const ZSTD: &str = "zstd";

/// Axum middleware applying the negotiation above.
pub async fn zstd_content_encoding(request: Request, next: Next) -> Response {
    zstd_content_encoding_with_limit(request, next, MAX_BODY_BYTES).await
}

pub(crate) async fn zstd_content_encoding_with_limit(
    request: Request,
    next: Next,
    max_body_bytes: usize,
) -> Response {
    let accepts_zstd = accepts_zstd(request.headers());
    let head_request = request.method() == Method::HEAD;
    let request = match decode_request(request, max_body_bytes).await {
        Ok(request) => request,
        Err(refused) => return advertise(refused.into_response()),
    };
    let response = next.run(request).await;
    let response = if accepts_zstd && !head_request {
        encode_response(response).await
    } else {
        response
    };
    advertise(response)
}

fn advertise(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(ACCEPT_ENCODING, HeaderValue::from_static(ZSTD));
    response
}

/// A refused request body: status plus message, rendered like `AppError`.
#[derive(Debug)]
pub struct CodingError {
    pub status: StatusCode,
    pub message: String,
}

impl IntoResponse for CodingError {
    fn into_response(self) -> Response {
        (self.status, axum::Json(json!({ "error": self.message }))).into_response()
    }
}

fn error(status: StatusCode, message: &str) -> CodingError {
    CodingError {
        status,
        message: message.to_owned(),
    }
}

/// The request's single content coding, lowercased; `None` for identity.
fn request_coding(headers: &HeaderMap) -> Result<Option<String>, CodingError> {
    let Some(value) = headers.get(CONTENT_ENCODING) else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid Content-Encoding header"))?;
    let codings: Vec<String> = value
        .split(',')
        .map(|c| c.trim().to_ascii_lowercase())
        .filter(|c| !c.is_empty() && c != "identity")
        .collect();
    match codings.as_slice() {
        [] => Ok(None),
        [one] if one == ZSTD => Ok(Some(one.clone())),
        _ => Err(error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported Content-Encoding; this server accepts zstd",
        )),
    }
}

async fn decode_request(request: Request, max_body_bytes: usize) -> Result<Request, CodingError> {
    if request_coding(request.headers())?.is_none() {
        return Ok(request);
    }
    let (mut parts, body) = request.into_parts();
    let compressed = to_bytes(body, max_body_bytes).await.map_err(|_| {
        error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "request body exceeds byte limit",
        )
    })?;
    let decoded = tokio::task::spawn_blocking(move || decode_zstd(&compressed, max_body_bytes))
        .await
        .map_err(|_| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "decompression task failed",
            )
        })??;
    parts.headers.remove(CONTENT_ENCODING);
    parts
        .headers
        .insert(CONTENT_LENGTH, HeaderValue::from(decoded.len()));
    Ok(Request::from_parts(parts, Body::from(decoded)))
}

/// Decode at most `limit` bytes; a frame that expands past it is refused
/// without materialising the rest.
pub fn decode_zstd(compressed: &[u8], limit: usize) -> Result<Vec<u8>, CodingError> {
    let decoder = ruzstd::decoding::StreamingDecoder::new(Cursor::new(compressed))
        .map_err(|e| error(StatusCode::BAD_REQUEST, &format!("invalid zstd body: {e}")))?;
    let mut out = Vec::new();
    decoder
        .take(limit as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| error(StatusCode::BAD_REQUEST, &format!("invalid zstd body: {e}")))?;
    if out.len() > limit {
        return Err(error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "decompressed request body exceeds byte limit",
        ));
    }
    Ok(out)
}

/// Whether `Accept-Encoding` lists zstd with a non-zero quality (or `*`
/// without an explicit zstd refusal).
pub fn accepts_zstd(headers: &HeaderMap) -> bool {
    let mut wildcard = false;
    for value in headers.get_all(ACCEPT_ENCODING) {
        let Ok(value) = value.to_str() else { continue };
        for item in value.split(',') {
            let mut fields = item.split(';');
            let coding = fields.next().unwrap_or("").trim().to_ascii_lowercase();
            let quality = fields
                .filter_map(|p| p.trim().strip_prefix("q="))
                .find_map(|q| q.trim().parse::<f32>().ok())
                .unwrap_or(1.0);
            if coding == ZSTD {
                return quality > 0.0;
            }
            if coding == "*" && quality > 0.0 {
                wildcard = true;
            }
        }
    }
    wildcard
}

fn compressible(headers: &HeaderMap) -> bool {
    if headers.contains_key(CONTENT_ENCODING) {
        return false;
    }
    let Some(content_type) = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let content_type = content_type.to_ascii_lowercase();
    content_type.starts_with("application/json") || content_type.starts_with("text/")
}

async fn encode_response(response: Response) -> Response {
    let status = response.status();
    if status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED
        || status.is_informational()
        || !compressible(response.headers())
    {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let bytes: Bytes = match to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return error(StatusCode::INTERNAL_SERVER_ERROR, "failed to read response")
                .into_response();
        }
    };
    parts
        .headers
        .append(VARY, HeaderValue::from_static("accept-encoding"));
    if bytes.len() < MIN_COMPRESS_BYTES {
        return Response::from_parts(parts, Body::from(bytes));
    }
    let original = bytes.clone();
    let encoded = tokio::task::spawn_blocking(move || {
        ruzstd::encoding::compress_to_vec(
            Cursor::new(bytes),
            ruzstd::encoding::CompressionLevel::Fastest,
        )
    })
    .await;
    match encoded {
        Ok(encoded) if encoded.len() < original.len() => {
            parts
                .headers
                .insert(CONTENT_ENCODING, HeaderValue::from_static(ZSTD));
            parts
                .headers
                .insert(CONTENT_LENGTH, HeaderValue::from(encoded.len()));
            Response::from_parts(parts, Body::from(encoded))
        }
        _ => Response::from_parts(parts, Body::from(original)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::routing::post;
    use tower::ServiceExt;

    fn app(limit: usize) -> Router {
        Router::new()
            .route(
                "/echo",
                post(|body: Bytes| async move {
                    (
                        [(CONTENT_TYPE, "application/json")],
                        format!(
                            "{{\"len\":{},\"pad\":\"{}\"}}",
                            body.len(),
                            "x".repeat(4096)
                        ),
                    )
                }),
            )
            .route(
                "/small",
                post(|| async { ([(CONTENT_TYPE, "application/json")], "{\"ok\":true}") }),
            )
            .layer(axum::middleware::from_fn(move |req, next| {
                zstd_content_encoding_with_limit(req, next, limit)
            }))
    }

    fn zstd(data: &[u8]) -> Vec<u8> {
        ruzstd::encoding::compress_to_vec(
            Cursor::new(data.to_vec()),
            ruzstd::encoding::CompressionLevel::Fastest,
        )
    }

    async fn body_of(response: Response) -> Vec<u8> {
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    fn post_req(uri: &str, body: Vec<u8>, headers: &[(&str, &str)]) -> Request {
        let mut builder = Request::builder().method("POST").uri(uri);
        for (k, v) in headers {
            builder = builder.header(*k, *v);
        }
        builder.body(Body::from(body)).unwrap()
    }

    #[tokio::test]
    async fn decodes_zstd_request_bodies() {
        let payload = vec![b'a'; 50_000];
        let res = app(1 << 20)
            .oneshot(post_req(
                "/echo",
                zstd(&payload),
                &[("content-encoding", "zstd")],
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[ACCEPT_ENCODING], "zstd");
        let body = String::from_utf8(body_of(res).await).unwrap();
        assert!(body.starts_with("{\"len\":50000,"), "{body}");
    }

    #[tokio::test]
    async fn identity_requests_pass_through_and_advertise() {
        let res = app(1 << 20)
            .oneshot(post_req("/small", b"{}".to_vec(), &[]))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[ACCEPT_ENCODING], "zstd");
        assert!(res.headers().get(CONTENT_ENCODING).is_none());
    }

    #[tokio::test]
    async fn rejects_bodies_that_expand_past_the_limit() {
        let bomb = zstd(&vec![0u8; 2 << 20]);
        assert!(bomb.len() < 4096);
        let res = app(1 << 20)
            .oneshot(post_req("/echo", bomb, &[("content-encoding", "zstd")]))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn rejects_compressed_bodies_over_the_limit() {
        let res = app(16)
            .oneshot(post_req(
                "/echo",
                vec![0u8; 64],
                &[("content-encoding", "zstd")],
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn rejects_invalid_zstd_and_unknown_codings() {
        let res = app(1 << 20)
            .oneshot(post_req(
                "/echo",
                b"not zstd".to_vec(),
                &[("content-encoding", "zstd")],
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let res = app(1 << 20)
            .oneshot(post_req(
                "/echo",
                b"{}".to_vec(),
                &[("content-encoding", "gzip")],
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(res.headers()[ACCEPT_ENCODING], "zstd");
        let res = app(1 << 20)
            .oneshot(post_req(
                "/small",
                b"{}".to_vec(),
                &[("content-encoding", "identity")],
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn encodes_large_json_responses_when_accepted() {
        let res = app(1 << 20)
            .oneshot(post_req(
                "/echo",
                b"{}".to_vec(),
                &[("accept-encoding", "gzip, zstd")],
            ))
            .await
            .unwrap();
        assert_eq!(res.headers()[CONTENT_ENCODING], "zstd");
        assert_eq!(res.headers()[VARY], "accept-encoding");
        let encoded = body_of(res).await;
        let decoded = decode_zstd(&encoded, 1 << 20).unwrap();
        assert!(
            String::from_utf8(decoded)
                .unwrap()
                .starts_with("{\"len\":2,")
        );
    }

    #[tokio::test]
    async fn leaves_small_or_unaccepted_responses_alone() {
        let res = app(1 << 20)
            .oneshot(post_req(
                "/small",
                b"{}".to_vec(),
                &[("accept-encoding", "zstd")],
            ))
            .await
            .unwrap();
        assert!(res.headers().get(CONTENT_ENCODING).is_none());
        let res = app(1 << 20)
            .oneshot(post_req(
                "/echo",
                b"{}".to_vec(),
                &[("accept-encoding", "gzip")],
            ))
            .await
            .unwrap();
        assert!(res.headers().get(CONTENT_ENCODING).is_none());
        let res = app(1 << 20)
            .oneshot(post_req(
                "/echo",
                b"{}".to_vec(),
                &[("accept-encoding", "zstd;q=0, *")],
            ))
            .await
            .unwrap();
        assert!(res.headers().get(CONTENT_ENCODING).is_none());
    }

    #[test]
    fn parses_accept_encoding() {
        let mut headers = HeaderMap::new();
        assert!(!accepts_zstd(&headers));
        headers.insert(
            ACCEPT_ENCODING,
            HeaderValue::from_static("br;q=1, ZSTD;q=0.5"),
        );
        assert!(accepts_zstd(&headers));
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("*;q=0.1"));
        assert!(accepts_zstd(&headers));
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("zstd;q=0"));
        assert!(!accepts_zstd(&headers));
    }
}
