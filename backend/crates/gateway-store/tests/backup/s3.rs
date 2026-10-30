use std::io;
use std::time::Duration;

use chrono::Utc;
use gateway_admin::model::backup::{BackupObjectMetadata, BackupStorageConfig, code};
use gateway_admin::ports::backup::{BackupObjectStorePort, UploadObjectRequest};
use gateway_core::lifecycle::CancellationToken;
use gateway_store::backup::s3::S3ObjectStoreAdapter;
use secrecy::SecretString;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

const PART_SIZE_ABOVE_TOKIO_DEFAULT: usize = 2 * 1024 * 1024 + 1;

#[tokio::test]
async fn upload_file_sends_full_part_beyond_tokio_default_buffer_limit() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let source = directory.path().join("archive.dump");
    let expected = vec![b'x'; PART_SIZE_ABOVE_TOKIO_DEFAULT];
    tokio::fs::write(&source, &expected)
        .await
        .expect("write temporary archive");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind multipart probe server");
    let endpoint = format!(
        "http://{}",
        listener.local_addr().expect("read listener address")
    );
    let server = tokio::spawn(serve_multipart_probe(listener));
    let storage = BackupStorageConfig {
        storage_revision: 1,
        endpoint,
        region: "auto".to_owned(),
        bucket: "backup-bucket".to_owned(),
        access_key_id: "test-access-key".to_owned(),
        secret_access_key: SecretString::from("test-secret-key"),
        prefix: "backups".to_owned(),
        force_path_style: true,
    };
    let request = UploadObjectRequest {
        object_key: "backups/archive.dump".to_owned(),
        source,
        metadata: BackupObjectMetadata::new(
            "backup_test".to_owned(),
            "a".repeat(64),
            Utc::now(),
            PART_SIZE_ABOVE_TOKIO_DEFAULT as u64,
        ),
        cancellation: CancellationToken::new(),
    };

    S3ObjectStoreAdapter::new()
        .upload_file(&storage, request)
        .await
        .expect("upload multipart archive");
    let uploaded_len = server
        .await
        .expect("multipart probe task did not panic")
        .expect("serve multipart probe");

    assert_eq!(uploaded_len, PART_SIZE_ABOVE_TOKIO_DEFAULT);
}

#[tokio::test]
async fn connection_probe_attempts_cleanup_after_an_interrupted_get_body() {
    for delete_allowed in [true, false] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let storage = BackupStorageConfig {
            storage_revision: 1,
            endpoint: format!("http://{}", listener.local_addr().unwrap()),
            region: "auto".into(),
            bucket: "backup-bucket".into(),
            access_key_id: "test-access-key".into(),
            secret_access_key: SecretString::from("test-secret-key"),
            prefix: "backups".into(),
            force_path_style: true,
        };
        let mut probe =
            tokio::spawn(
                async move { S3ObjectStoreAdapter::new().test_connection(&storage).await },
            );
        let (mut put, request, body) = accept_request(&listener).await;
        assert!(request.starts_with("PUT "));
        let object_path = request
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .split('?')
            .next()
            .unwrap()
            .to_owned();
        respond(&mut put, &xml_response("")).await;
        let (mut head, request, _) = accept_request(&listener).await;
        assert!(request.starts_with("HEAD "));
        respond(&mut head, "HTTP/1.1 200 OK\r\ncontent-length: 64\r\nx-amz-meta-probe: backup-connection-test\r\nconnection: close\r\n\r\n").await;
        let (mut get, request, _) = accept_request(&listener).await;
        assert!(request.starts_with("GET "));
        get.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 64\r\nconnection: close\r\n\r\n")
            .await
            .unwrap();
        get.write_all(&body[..32]).await.unwrap();
        get.shutdown().await.unwrap();
        drop(get);
        let (mut delete, request, _) = tokio::select! {
            result = &mut probe => panic!("probe returned before cleaning up the uploaded object: {result:?}"),
            request = accept_request(&listener) => request,
        };
        assert!(request.starts_with("DELETE "));
        let delete_path = request
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .split('?')
            .next()
            .unwrap();
        assert_eq!(delete_path, object_path);
        respond(
            &mut delete,
            &if delete_allowed {
                xml_response("")
            } else {
                error_response("403 Forbidden", "AccessDenied")
            },
        )
        .await;
        let result = tokio::time::timeout(Duration::from_secs(3), probe)
            .await
            .unwrap()
            .unwrap()
            .expect("body failure is a staged connection test result");
        assert!(!result.ok);
        assert_eq!(result.stage, "getObject");
        assert_eq!(result.code, Some(code::UPSTREAM_FAILURE));
        assert_eq!(result.message, "读取探针对象响应失败");
    }
}

#[tokio::test]
async fn cancelling_multipart_upload_drops_part_retries_before_remote_abort() {
    check_cancelled_multipart(false).await;
    check_cancelled_multipart(true).await;
}

async fn check_cancelled_multipart(abort_fails: bool) {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("archive.dump");
    let size = 16 * 1024 * 1024 + 1;
    tokio::fs::File::create(&source)
        .await
        .unwrap()
        .set_len(size)
        .await
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut create, _) = listener.accept().await.unwrap();
        let (request, _) = read_http_request(&mut create).await.unwrap();
        assert!(request.starts_with("POST ") && request.contains("uploads"));
        create.write_all(xml_response("<InitiateMultipartUploadResult><UploadId>cancel-probe</UploadId></InitiateMultipartUploadResult>").as_bytes()).await.unwrap();
        create.shutdown().await.unwrap();
        let mut pending = Vec::new();
        for _ in 0..2 {
            let (mut part, _) = listener.accept().await.unwrap();
            let (request, _) = read_http_request(&mut part).await.unwrap();
            assert!(request.starts_with("PUT ") && request.contains("partNumber="));
            pending.push(part);
        }
        started_tx.send(()).unwrap();
        let (mut abort, _) = listener.accept().await.unwrap();
        let (request, _) = read_http_request(&mut abort).await.unwrap();
        assert!(request.starts_with("DELETE ") && request.contains("uploadId="));
        let response = if abort_fails {
            let body =
                "<Error><Code>AccessDenied</Code><Message>sensitive abort detail</Message></Error>";
            format!(
                "HTTP/1.1 403 Forbidden\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
        } else {
            "HTTP/1.1 204 No Content\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_owned()
        };
        abort.write_all(response.as_bytes()).await.unwrap();
        abort.shutdown().await.unwrap();
        // A detached SDK request would retry these failures after remote cleanup.
        for mut part in pending {
            let _ = part.write_all(b"HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
            let _ = part.shutdown().await;
        }
        assert!(
            tokio::time::timeout(Duration::from_secs(3), listener.accept())
                .await
                .is_err(),
            "part retries or completion must not outlive cancellation"
        );
    });
    let cancellation = CancellationToken::new();
    let request = UploadObjectRequest {
        object_key: "backups/cancel.dump".to_owned(),
        source,
        metadata: BackupObjectMetadata::new(
            "backup_cancel".to_owned(),
            "b".repeat(64),
            Utc::now(),
            size,
        ),
        cancellation: cancellation.clone(),
    };
    let storage = BackupStorageConfig {
        storage_revision: 1,
        endpoint,
        region: "auto".to_owned(),
        bucket: "backup-bucket".to_owned(),
        access_key_id: "test-access-key".to_owned(),
        secret_access_key: SecretString::from("test-secret-key"),
        prefix: "backups".to_owned(),
        force_path_style: true,
    };
    let upload = tokio::spawn(async move {
        S3ObjectStoreAdapter::new()
            .upload_file(&storage, request)
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), started_rx)
        .await
        .unwrap()
        .unwrap();
    cancellation.cancel();
    let error = tokio::time::timeout(Duration::from_secs(5), upload)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code(), gateway_admin::model::backup::code::CANCELLED);
    assert_eq!(
        error.message().contains("AbortMultipartUpload"),
        abort_fails
    );
    assert!(!error.message().contains("sensitive abort detail"));
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

async fn serve_multipart_probe(listener: TcpListener) -> io::Result<usize> {
    let mut uploaded_len = None;
    for _ in 0..3 {
        let (mut stream, _) = listener.accept().await?;
        let (request, body) = read_http_request(&mut stream).await?;
        let response = if request.starts_with("POST ") && request.contains("uploads") {
            xml_response(
                "<InitiateMultipartUploadResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><UploadId>probe-upload</UploadId></InitiateMultipartUploadResult>",
            )
        } else if request.starts_with("PUT ") && request.contains("partNumber=") {
            uploaded_len = Some(body.len());
            "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\netag: \"probe-part\"\r\n\r\n".to_owned()
        } else if request.starts_with("POST ") && request.contains("uploadId=") {
            xml_response(
                "<CompleteMultipartUploadResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><ETag>\"probe-part\"</ETag></CompleteMultipartUploadResult>",
            )
        } else {
            return Err(io::Error::other("unexpected multipart probe request"));
        };
        stream.write_all(response.as_bytes()).await?;
        stream.shutdown().await?;
    }
    uploaded_len.ok_or_else(|| io::Error::other("multipart probe did not receive an upload part"))
}

#[tokio::test]
async fn connection_probe_requires_delete_permission() {
    for allowed in [true, false] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let storage = BackupStorageConfig {
            storage_revision: 1,
            endpoint: format!("http://{}", listener.local_addr().unwrap()),
            region: "auto".to_owned(),
            bucket: "backup-bucket".to_owned(),
            access_key_id: "test-access-key".to_owned(),
            secret_access_key: SecretString::from("test-secret-key"),
            prefix: "backups".to_owned(),
            force_path_style: true,
        };
        let server = tokio::spawn(async move {
            let mut probe_body = Vec::new();
            for method in ["PUT", "HEAD", "GET", "DELETE"] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (request, body) = read_http_request(&mut stream).await.unwrap();
                assert!(request.starts_with(method));
                let response = match method {
                    "PUT" => { probe_body = body; xml_response("") },
                    "HEAD" => "HTTP/1.1 200 OK\r\ncontent-length: 64\r\nx-amz-meta-probe: backup-connection-test\r\nconnection: close\r\n\r\n".to_owned(),
                    "GET" => format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", probe_body.len()),
                    _ if allowed => xml_response(""),
                    _ => {
                        let body = "<Error><Code>AccessDenied</Code><Message>synthetic failure</Message></Error>";
                        format!("HTTP/1.1 403 Forbidden\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len())
                    },
                };
                stream.write_all(response.as_bytes()).await.unwrap();
                if method == "GET" {
                    stream.write_all(&probe_body).await.unwrap();
                }
                stream.shutdown().await.unwrap();
            }
        });
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            S3ObjectStoreAdapter::new().test_connection(&storage),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result.ok, allowed);
        assert_eq!(result.stage, "deleteObject");
        assert_eq!(
            result.code,
            (!allowed).then_some(gateway_admin::model::backup::code::S3_PERMISSION_DENIED)
        );
        server.await.unwrap();
    }
}

async fn read_http_request(stream: &mut TcpStream) -> io::Result<(String, Vec<u8>)> {
    let mut bytes = Vec::new();
    let header_end = loop {
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        let mut chunk = [0_u8; 8 * 1024];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "multipart probe request ended before headers",
            ));
        }
        bytes.extend_from_slice(&chunk[..read]);
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]).into_owned();
    let content_len = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or_default();
    if headers
        .to_ascii_lowercase()
        .contains("expect: 100-continue")
    {
        stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").await?;
    }
    while bytes.len() < header_end + content_len {
        let mut chunk = [0_u8; 8 * 1024];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "multipart probe request ended before body",
            ));
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    Ok((
        headers,
        bytes[header_end..header_end + content_len].to_vec(),
    ))
}

fn xml_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/xml\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}
async fn accept_request(listener: &TcpListener) -> (TcpStream, String, Vec<u8>) {
    tokio::time::timeout(Duration::from_secs(3), async {
        let (mut stream, _) = listener.accept().await.expect("accept S3 request");
        let (request, body) = read_http_request(&mut stream)
            .await
            .expect("read S3 request");
        (stream, request, body)
    })
    .await
    .expect("expected S3 request arrives without waiting for another part")
}

async fn respond(stream: &mut TcpStream, response: &str) {
    stream.write_all(response.as_bytes()).await.unwrap();
    stream.shutdown().await.unwrap();
}

fn error_response(status: &str, code: &str) -> String {
    let body = format!("<Error><Code>{code}</Code><Message>probe error</Message></Error>");
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/xml\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}
