use std::io;
use std::time::Duration;

use chrono::Utc;
use gateway_admin::model::backup::{BackupObjectMetadata, BackupStorageConfig};
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
async fn cancelling_multipart_upload_drops_part_retries_before_remote_abort() {
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
        abort
            .write_all(b"HTTP/1.1 204 No Content\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
            .await
            .unwrap();
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
