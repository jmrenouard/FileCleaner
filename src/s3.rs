//! Pure-Rust S3 / S3-compatible archival client with AWS Signature Version 4
//! and mandatory fail-closed SHA-256 post-upload verification (`IDEA-001`).

use crate::actions::open_regular_nofollow_read;
use crate::config::{S3Config, S3VerifyMode};
use crate::error::{FileCleanerError, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use chrono::format::{Item, StrftimeItems};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::{Agent, SendBody};

type HmacSha256 = Hmac<Sha256>;

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const STREAM_BUF_SIZE: usize = 64 * 1024;
const MAX_MULTIPART_PARTS: u64 = 10_000;
const DEFAULT_BASE_BACKOFF: Duration = Duration::from_millis(200);
const MAX_BACKOFF: Duration = Duration::from_secs(5);
const MAX_ERROR_BODY_BYTES: usize = 4096;

/// Redacting secret string wrapper that never prints plaintext in `Debug` or `Display`.
#[derive(Clone, PartialEq, Eq)]
struct RedactedSecret(String);

impl RedactedSecret {
    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RedactedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl fmt::Display for RedactedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// S3 credentials resolved strictly from environment variables.
#[derive(Clone, PartialEq, Eq)]
struct S3Credentials {
    access_key_id: String,
    secret_access_key: RedactedSecret,
    session_token: Option<RedactedSecret>,
}

impl fmt::Debug for S3Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Credentials")
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &self.secret_access_key)
            .field("session_token", &self.session_token)
            .finish()
    }
}

impl S3Credentials {
    fn from_env(cfg: &S3Config, path: &Path, uri: &str) -> Result<Self> {
        let access_key_id = std::env::var(&cfg.access_key_env)
            .ok()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| FileCleanerError::S3Upload {
                path: path.to_path_buf(),
                uri: uri.to_string(),
                reason: format!(
                    "missing or empty S3 access key environment variable '{}'",
                    cfg.access_key_env
                ),
            })?;
        let secret_raw = std::env::var(&cfg.secret_key_env)
            .ok()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| FileCleanerError::S3Upload {
                path: path.to_path_buf(),
                uri: uri.to_string(),
                reason: format!(
                    "missing or empty S3 secret key environment variable '{}'",
                    cfg.secret_key_env
                ),
            })?;
        let session_token = cfg
            .session_token_env
            .as_deref()
            .and_then(|name| std::env::var(name).ok())
            .filter(|v| !v.is_empty())
            .map(RedactedSecret);

        Ok(Self {
            access_key_id,
            secret_access_key: RedactedSecret(secret_raw),
            session_token,
        })
    }
}

/// Input parameters for a single fail-closed S3 archival upload.
#[derive(Debug, Clone)]
pub struct S3UploadRequest {
    /// Local candidate file path to read and verify.
    pub local_path: PathBuf,
    /// Target object key inside `config.bucket` (no leading slash).
    pub key: String,
    /// Exact local byte length computed from the candidate file.
    pub content_length: u64,
    /// Lowercase hexadecimal SHA-256 digest of the candidate file.
    pub sha256_hex: String,
    /// Effective S3 sink configuration.
    pub config: S3Config,
}

/// Verified archival receipt returned only after post-upload SHA-256 verification succeeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3UploadReceipt {
    /// Canonical S3 object URI (`s3://<bucket>/<key>`).
    pub uri: String,
    /// Remote `ETag` header returned by the S3 server, if present.
    pub etag: Option<String>,
    /// Verified lowercase hex SHA-256 digest matching the local artifact.
    pub verified_sha256: String,
    /// Verified remote `Content-Length` equal to local byte length.
    pub verified_size: u64,
    /// Verification strategy enforced (`ChecksumSha256` or `ReadbackSha256`).
    pub verify_mode: S3VerifyMode,
    /// Number of uploaded object parts (`1` for single-part `PutObject`).
    pub parts_uploaded: usize,
}

/// Injectable archival sink abstraction enabling deterministic offline and loopback testing.
pub trait S3Uploader: Send + Sync {
    /// Uploads `req.local_path` to S3 and verifies remote size and SHA-256 fail-closed.
    fn upload_and_verify(&self, req: &S3UploadRequest) -> Result<S3UploadReceipt>;
}

/// Production pure-Rust S3 client backed by `ureq` v3 + `rustls` v0.23 + `rustls-rustcrypto`.
#[derive(Clone)]
pub struct HttpS3Uploader {
    crypto_provider: Arc<rustls::crypto::CryptoProvider>,
    base_backoff: Duration,
    #[cfg(test)]
    multipart_threshold_override: Option<u64>,
}

impl fmt::Debug for HttpS3Uploader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpS3Uploader")
            .field("base_backoff", &self.base_backoff)
            .finish()
    }
}

impl Default for HttpS3Uploader {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpS3Uploader {
    /// Constructs a new pure-Rust S3 uploader using `rustls-rustcrypto`.
    pub fn new() -> Self {
        Self::with_base_backoff(DEFAULT_BASE_BACKOFF)
    }

    /// Constructs an uploader with a custom retry base backoff duration.
    pub fn with_base_backoff(base_backoff: Duration) -> Self {
        Self {
            crypto_provider: Arc::new(rustls_rustcrypto::provider()),
            base_backoff,
            #[cfg(test)]
            multipart_threshold_override: None,
        }
    }

    #[cfg(test)]
    fn with_multipart_override(mut self, part_size: u64) -> Self {
        self.multipart_threshold_override = Some(part_size);
        self
    }

    fn build_agent(&self, cfg: &S3Config) -> Agent {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::WebPki)
            .unversioned_rustls_crypto_provider(Arc::clone(&self.crypto_provider))
            .build();
        Agent::config_builder()
            .timeout_global(Some(cfg.timeout))
            .http_status_as_error(false)
            .https_only(!cfg.allow_insecure_http)
            .max_redirects(0)
            .tls_config(tls)
            .build()
            .new_agent()
    }

    fn effective_part_size(&self, cfg: &S3Config) -> u64 {
        #[cfg(test)]
        if let Some(override_sz) = self.multipart_threshold_override {
            return override_sz;
        }
        cfg.multipart_part_size
    }

    fn sleep_backoff(&self, attempt: u32) {
        if self.base_backoff.is_zero() {
            return;
        }
        let mult = 1u32.checked_shl(attempt.min(6)).unwrap_or(64);
        let delay = self.base_backoff.saturating_mul(mult).min(MAX_BACKOFF);
        thread::sleep(delay);
    }

    fn retry_op<T, F>(&self, max_retries: u32, mut op: F) -> Result<T>
    where
        F: FnMut() -> std::result::Result<T, RetryableError>,
    {
        let mut attempt = 0u32;
        loop {
            match op() {
                Ok(val) => return Ok(val),
                Err(RetryableError::Transient(err)) if attempt < max_retries => {
                    self.sleep_backoff(attempt);
                    attempt += 1;
                    let _ = err;
                }
                Err(RetryableError::Transient(err) | RetryableError::Fatal(err)) => {
                    return Err(err);
                }
            }
        }
    }

    fn check_not_exists(
        &self,
        agent: &Agent,
        ep: &ResolvedEndpoint,
        creds: &S3Credentials,
        req: &S3UploadRequest,
        uri: &str,
    ) -> Result<()> {
        self.retry_op(req.config.max_retries, || {
            let signed = sign_v4_request(
                "HEAD",
                &ep.canonical_uri,
                &[],
                &[("host", ep.host_header.as_str())],
                EMPTY_SHA256,
                creds,
                &req.config.region,
                Utc::now(),
            );
            let mut builder = agent.head(&ep.url_base);
            for (k, v) in &signed.headers {
                builder = builder.header(k.as_str(), v.as_str());
            }
            let resp = builder
                .header("authorization", signed.authorization.as_str())
                .call()
                .map_err(|e| {
                    RetryableError::Transient(s3_err(
                        &req.local_path,
                        uri,
                        format!("HEAD preflight failed: {e}"),
                    ))
                })?;

            let status = resp.status().as_u16();
            match status {
                404 => Ok(()),
                200 => Err(RetryableError::Fatal(FileCleanerError::io(
                    &req.local_path,
                    io::Error::new(
                        ErrorKind::AlreadyExists,
                        format!(
                            "remote S3 object already exists at '{uri}' and overwrite is false"
                        ),
                    ),
                ))),
                500..=599 => Err(RetryableError::Transient(s3_err(
                    &req.local_path,
                    uri,
                    format!("HEAD preflight server error HTTP {status}"),
                ))),
                _ => Err(RetryableError::Fatal(s3_err(
                    &req.local_path,
                    uri,
                    format!("HEAD preflight rejected with HTTP {status}"),
                ))),
            }
        })
    }

    fn upload_single_part(
        &self,
        agent: &Agent,
        ep: &ResolvedEndpoint,
        creds: &S3Credentials,
        req: &S3UploadRequest,
        uri: &str,
        sha256_b64: &str,
    ) -> Result<()> {
        self.retry_op(req.config.max_retries, || {
            let (file, meta) =
                open_regular_nofollow_read(&req.local_path).map_err(RetryableError::Fatal)?;
            if meta.len() != req.content_length {
                return Err(RetryableError::Fatal(FileCleanerError::IntegrityMismatch {
                    path: req.local_path.clone(),
                    expected: format!("{} ({} bytes)", req.sha256_hex, req.content_length),
                    actual: format!("file size changed before upload ({} bytes)", meta.len()),
                }));
            }

            let mut hdrs: Vec<(&str, &str)> = vec![
                ("host", ep.host_header.as_str()),
                ("content-type", "application/octet-stream"),
                ("x-amz-checksum-algorithm", "SHA256"),
                ("x-amz-checksum-sha256", sha256_b64),
                ("x-amz-meta-filecleaner-sha256", req.sha256_hex.as_str()),
            ];
            if let Some(sc) = req.config.storage_class.as_deref() {
                hdrs.push(("x-amz-storage-class", sc));
            }
            let signed = sign_v4_request(
                "PUT",
                &ep.canonical_uri,
                &[],
                &hdrs,
                &req.sha256_hex,
                creds,
                &req.config.region,
                Utc::now(),
            );

            let mut builder = agent
                .put(&ep.url_base)
                .header("content-length", req.content_length.to_string());
            for (k, v) in &signed.headers {
                builder = builder.header(k.as_str(), v.as_str());
            }
            let body = SendBody::from_owned_reader(file.take(req.content_length));
            let resp = builder
                .header("authorization", signed.authorization.as_str())
                .send(body)
                .map_err(|e| {
                    RetryableError::Transient(s3_err(
                        &req.local_path,
                        uri,
                        format!("PutObject transport failed: {e}"),
                    ))
                })?;

            classify_http_status(resp, "PutObject", &req.local_path, uri).map(|_| ())
        })
    }

    fn upload_multipart(
        &self,
        agent: &Agent,
        ep: &ResolvedEndpoint,
        creds: &S3Credentials,
        req: &S3UploadRequest,
        uri: &str,
        part_size: u64,
    ) -> Result<(usize, String)> {
        let total_parts = req.content_length.div_ceil(part_size);
        if total_parts == 0 || total_parts > MAX_MULTIPART_PARTS {
            return Err(s3_err(
                &req.local_path,
                uri,
                format!(
                    "multipart upload requires {total_parts} parts (S3 limit is 1..={MAX_MULTIPART_PARTS}); increase multipart_part_size"
                ),
            ));
        }

        let upload_id = self.create_multipart_upload(agent, ep, creds, req, uri)?;

        let result = self.upload_and_complete_parts(
            agent,
            ep,
            creds,
            req,
            uri,
            &upload_id,
            part_size,
            total_parts as usize,
        );

        if result.is_err() {
            self.abort_multipart_upload(agent, ep, creds, req, &upload_id);
        }

        result
    }

    fn create_multipart_upload(
        &self,
        agent: &Agent,
        ep: &ResolvedEndpoint,
        creds: &S3Credentials,
        req: &S3UploadRequest,
        uri: &str,
    ) -> Result<String> {
        self.retry_op(req.config.max_retries, || {
            let query = [("uploads", "")];
            let mut hdrs: Vec<(&str, &str)> = vec![
                ("host", ep.host_header.as_str()),
                ("content-type", "application/octet-stream"),
                ("x-amz-checksum-algorithm", "SHA256"),
                ("x-amz-meta-filecleaner-sha256", req.sha256_hex.as_str()),
            ];
            if let Some(sc) = req.config.storage_class.as_deref() {
                hdrs.push(("x-amz-storage-class", sc));
            }
            let signed = sign_v4_request(
                "POST",
                &ep.canonical_uri,
                &query,
                &hdrs,
                EMPTY_SHA256,
                creds,
                &req.config.region,
                Utc::now(),
            );
            let url = format!("{}?uploads=", ep.url_base);
            let mut builder = agent.post(&url);
            for (k, v) in &signed.headers {
                builder = builder.header(k.as_str(), v.as_str());
            }
            let resp = builder
                .header("authorization", signed.authorization.as_str())
                .send_empty()
                .map_err(|e| {
                    RetryableError::Transient(s3_err(
                        &req.local_path,
                        uri,
                        format!("CreateMultipartUpload transport failed: {e}"),
                    ))
                })?;

            let mut ok_resp =
                classify_http_status(resp, "CreateMultipartUpload", &req.local_path, uri)?;
            let body_xml = read_bounded_body_string(&mut ok_resp, MAX_ERROR_BODY_BYTES);
            xml_extract_tag(&body_xml, "UploadId").ok_or_else(|| {
                RetryableError::Fatal(s3_err(
                    &req.local_path,
                    uri,
                    "CreateMultipartUpload response missing <UploadId>",
                ))
            })
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn upload_and_complete_parts(
        &self,
        agent: &Agent,
        ep: &ResolvedEndpoint,
        creds: &S3Credentials,
        req: &S3UploadRequest,
        uri: &str,
        upload_id: &str,
        part_size: u64,
        total_parts: usize,
    ) -> Result<(usize, String)> {
        let mut completed_parts: Vec<CompletedPart> = Vec::with_capacity(total_parts);
        let mut part_raw_hashes: Vec<[u8; 32]> = Vec::with_capacity(total_parts);
        let mut full_hasher = Sha256::new();

        for part_idx in 0..total_parts {
            let part_number = part_idx + 1;
            let offset = (part_idx as u64).saturating_mul(part_size);
            let part_len = (req.content_length - offset).min(part_size);

            let (part_raw_sha, part_hex, part_b64) =
                hash_file_range(&req.local_path, offset, part_len, &mut full_hasher)?;
            let part_num_str = part_number.to_string();

            let etag = self.retry_op(req.config.max_retries, || {
                let (mut file, _) =
                    open_regular_nofollow_read(&req.local_path).map_err(RetryableError::Fatal)?;
                file.seek(SeekFrom::Start(offset))
                    .map_err(|e| RetryableError::Fatal(FileCleanerError::io(&req.local_path, e)))?;

                let query = [
                    ("partNumber", part_num_str.as_str()),
                    ("uploadId", upload_id),
                ];
                let hdrs = [
                    ("host", ep.host_header.as_str()),
                    ("x-amz-checksum-algorithm", "SHA256"),
                    ("x-amz-checksum-sha256", part_b64.as_str()),
                ];
                let signed = sign_v4_request(
                    "PUT",
                    &ep.canonical_uri,
                    &query,
                    &hdrs,
                    &part_hex,
                    creds,
                    &req.config.region,
                    Utc::now(),
                );
                let url = format!("{}?{}", ep.url_base, canonical_query_string(&query));
                let mut builder = agent
                    .put(&url)
                    .header("content-length", part_len.to_string());
                for (k, v) in &signed.headers {
                    builder = builder.header(k.as_str(), v.as_str());
                }
                let resp = builder
                    .header("authorization", signed.authorization.as_str())
                    .send(SendBody::from_owned_reader(file.take(part_len)))
                    .map_err(|e| {
                        RetryableError::Transient(s3_err(
                            &req.local_path,
                            uri,
                            format!("UploadPart #{part_number} transport failed: {e}"),
                        ))
                    })?;

                let ok_resp = classify_http_status(
                    resp,
                    &format!("UploadPart #{part_number}"),
                    &req.local_path,
                    uri,
                )?;
                let etag = header_str(&ok_resp, "etag").ok_or_else(|| {
                    RetryableError::Fatal(s3_err(
                        &req.local_path,
                        uri,
                        format!("UploadPart #{part_number} response missing ETag"),
                    ))
                })?;
                Ok(etag)
            })?;

            part_raw_hashes.push(part_raw_sha);
            completed_parts.push(CompletedPart {
                part_number,
                etag,
                checksum_sha256_b64: part_b64,
            });
        }

        let streamed_full_hex = to_hex_lower(&full_hasher.finalize());
        if streamed_full_hex != req.sha256_hex {
            return Err(FileCleanerError::IntegrityMismatch {
                path: req.local_path.clone(),
                expected: req.sha256_hex.clone(),
                actual: streamed_full_hex,
            });
        }

        let mut xml_body = String::from("<CompleteMultipartUpload>");
        for part in &completed_parts {
            xml_body.push_str(&format!(
                "<Part><PartNumber>{}</PartNumber><ETag>{}</ETag><ChecksumSHA256>{}</ChecksumSHA256></Part>",
                part.part_number,
                xml_escape(&part.etag),
                xml_escape(&part.checksum_sha256_b64)
            ));
        }
        xml_body.push_str("</CompleteMultipartUpload>");
        let xml_sha_hex = sha256_bytes_hex(xml_body.as_bytes());

        self.retry_op(req.config.max_retries, || {
            let query = [("uploadId", upload_id)];
            let hdrs = [
                ("host", ep.host_header.as_str()),
                ("content-type", "application/xml"),
            ];
            let signed = sign_v4_request(
                "POST",
                &ep.canonical_uri,
                &query,
                &hdrs,
                &xml_sha_hex,
                creds,
                &req.config.region,
                Utc::now(),
            );
            let url = format!("{}?{}", ep.url_base, canonical_query_string(&query));
            let mut builder = agent
                .post(&url)
                .header("content-length", xml_body.len().to_string());
            for (k, v) in &signed.headers {
                builder = builder.header(k.as_str(), v.as_str());
            }
            let resp = builder
                .header("authorization", signed.authorization.as_str())
                .send(xml_body.as_bytes())
                .map_err(|e| {
                    RetryableError::Transient(s3_err(
                        &req.local_path,
                        uri,
                        format!("CompleteMultipartUpload transport failed: {e}"),
                    ))
                })?;

            let mut ok_resp =
                classify_http_status(resp, "CompleteMultipartUpload", &req.local_path, uri)?;
            let resp_xml = read_bounded_body_string(&mut ok_resp, MAX_ERROR_BODY_BYTES);
            // S3 may return HTTP 200 with an embedded <Error> element during completion.
            if resp_xml.contains("<Error>") {
                let code = xml_extract_tag(&resp_xml, "Code").unwrap_or_else(|| "Unknown".into());
                let msg = xml_extract_tag(&resp_xml, "Message").unwrap_or_default();
                return Err(RetryableError::Transient(s3_err(
                    &req.local_path,
                    uri,
                    format!("CompleteMultipartUpload embedded S3 error {code}: {msg}"),
                )));
            }
            Ok(())
        })?;

        let composite = compute_composite_multipart_checksum(&part_raw_hashes);
        Ok((total_parts, composite))
    }

    fn abort_multipart_upload(
        &self,
        agent: &Agent,
        ep: &ResolvedEndpoint,
        creds: &S3Credentials,
        req: &S3UploadRequest,
        upload_id: &str,
    ) {
        let query = [("uploadId", upload_id)];
        let hdrs = [("host", ep.host_header.as_str())];
        let signed = sign_v4_request(
            "DELETE",
            &ep.canonical_uri,
            &query,
            &hdrs,
            EMPTY_SHA256,
            creds,
            &req.config.region,
            Utc::now(),
        );
        let url = format!("{}?{}", ep.url_base, canonical_query_string(&query));
        let mut builder = agent.delete(&url);
        for (k, v) in &signed.headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        let _ = builder
            .header("authorization", signed.authorization.as_str())
            .call();
    }

    #[allow(clippy::too_many_arguments)]
    fn verify_uploaded_object(
        &self,
        agent: &Agent,
        ep: &ResolvedEndpoint,
        creds: &S3Credentials,
        req: &S3UploadRequest,
        uri: &str,
        expected_sha256_b64: &str,
        expected_composite: Option<&str>,
        parts_uploaded: usize,
    ) -> Result<S3UploadReceipt> {
        let (remote_size, etag, remote_checksum) = self.retry_op(req.config.max_retries, || {
            let hdrs = [
                ("host", ep.host_header.as_str()),
                ("x-amz-checksum-mode", "ENABLED"),
            ];
            let signed = sign_v4_request(
                "HEAD",
                &ep.canonical_uri,
                &[],
                &hdrs,
                EMPTY_SHA256,
                creds,
                &req.config.region,
                Utc::now(),
            );
            let mut builder = agent.head(&ep.url_base);
            for (k, v) in &signed.headers {
                builder = builder.header(k.as_str(), v.as_str());
            }
            let resp = builder
                .header("authorization", signed.authorization.as_str())
                .call()
                .map_err(|e| {
                    RetryableError::Transient(s3_err(
                        &req.local_path,
                        uri,
                        format!("HeadObject verification transport failed: {e}"),
                    ))
                })?;

            let ok_resp = classify_http_status(resp, "HeadObject verify", &req.local_path, uri)?;
            let remote_len = header_str(&ok_resp, "content-length")
                .and_then(|v| v.parse::<u64>().ok())
                .ok_or_else(|| {
                    RetryableError::Fatal(FileCleanerError::IntegrityMismatch {
                        path: req.local_path.clone(),
                        expected: format!("{} ({} bytes)", req.sha256_hex, req.content_length),
                        actual: format!("remote HeadObject at '{uri}' omitted Content-Length"),
                    })
                })?;
            let etag = header_str(&ok_resp, "etag");
            let remote_checksum = header_str(&ok_resp, "x-amz-checksum-sha256");
            Ok((remote_len, etag, remote_checksum))
        })?;

        if remote_size != req.content_length {
            return Err(FileCleanerError::IntegrityMismatch {
                path: req.local_path.clone(),
                expected: format!("{} ({} bytes)", req.sha256_hex, req.content_length),
                actual: format!("remote size mismatch at '{uri}' ({remote_size} bytes)"),
            });
        }

        match req.config.verify_mode {
            S3VerifyMode::ChecksumSha256 => {
                let Some(actual_hdr) = remote_checksum else {
                    return Err(FileCleanerError::IntegrityMismatch {
                        path: req.local_path.clone(),
                        expected: format!("x-amz-checksum-sha256 {expected_sha256_b64}"),
                        actual: format!(
                            "remote server at '{uri}' omitted x-amz-checksum-sha256 (set verify_mode = \"readback_sha256\" only for legacy gateways)"
                        ),
                    });
                };
                let matches_single = actual_hdr == expected_sha256_b64;
                let matches_composite =
                    expected_composite.is_some_and(|expected_c| actual_hdr == expected_c);
                if !matches_single && !matches_composite {
                    return Err(FileCleanerError::IntegrityMismatch {
                        path: req.local_path.clone(),
                        expected: match expected_composite {
                            Some(c) => format!("{expected_sha256_b64} or {c}"),
                            None => expected_sha256_b64.to_string(),
                        },
                        actual: actual_hdr,
                    });
                }
            }
            S3VerifyMode::ReadbackSha256 => {
                self.verify_readback_sha256(agent, ep, creds, req, uri)?;
            }
        }

        Ok(S3UploadReceipt {
            uri: uri.to_string(),
            etag,
            verified_sha256: req.sha256_hex.clone(),
            verified_size: remote_size,
            verify_mode: req.config.verify_mode,
            parts_uploaded,
        })
    }

    fn verify_readback_sha256(
        &self,
        agent: &Agent,
        ep: &ResolvedEndpoint,
        creds: &S3Credentials,
        req: &S3UploadRequest,
        uri: &str,
    ) -> Result<()> {
        let (actual_hex, actual_len) = self.retry_op(req.config.max_retries, || {
            let hdrs = [("host", ep.host_header.as_str())];
            let signed = sign_v4_request(
                "GET",
                &ep.canonical_uri,
                &[],
                &hdrs,
                EMPTY_SHA256,
                creds,
                &req.config.region,
                Utc::now(),
            );
            let mut builder = agent.get(&ep.url_base);
            for (k, v) in &signed.headers {
                builder = builder.header(k.as_str(), v.as_str());
            }
            let resp = builder
                .header("authorization", signed.authorization.as_str())
                .call()
                .map_err(|e| {
                    RetryableError::Transient(s3_err(
                        &req.local_path,
                        uri,
                        format!("GetObject readback transport failed: {e}"),
                    ))
                })?;

            let ok_resp = classify_http_status(resp, "GetObject readback", &req.local_path, uri)?;
            let mut reader = ok_resp.into_body().into_reader();
            let mut hasher = Sha256::new();
            let mut buf = [0u8; STREAM_BUF_SIZE];
            let mut total = 0u64;
            loop {
                let n = reader.read(&mut buf).map_err(|e| {
                    RetryableError::Transient(s3_err(
                        &req.local_path,
                        uri,
                        format!("GetObject readback stream failed: {e}"),
                    ))
                })?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                total = total.saturating_add(n as u64);
            }
            Ok((to_hex_lower(&hasher.finalize()), total))
        })?;

        if actual_hex != req.sha256_hex || actual_len != req.content_length {
            return Err(FileCleanerError::IntegrityMismatch {
                path: req.local_path.clone(),
                expected: format!("{} ({} bytes)", req.sha256_hex, req.content_length),
                actual: format!("{actual_hex} ({actual_len} bytes)"),
            });
        }
        Ok(())
    }
}

impl S3Uploader for HttpS3Uploader {
    fn upload_and_verify(&self, req: &S3UploadRequest) -> Result<S3UploadReceipt> {
        let uri = build_s3_uri(&req.config.bucket, &req.key);
        let raw_sha = decode_sha256_hex(&req.sha256_hex).ok_or_else(|| {
            s3_err(
                &req.local_path,
                &uri,
                format!("invalid local SHA-256 hex digest '{}'", req.sha256_hex),
            )
        })?;
        let sha256_b64 = BASE64_STANDARD.encode(raw_sha);
        let creds = S3Credentials::from_env(&req.config, &req.local_path, &uri)?;
        let ep = resolve_endpoint(&req.config, &req.key, &req.local_path, &uri)?;
        let agent = self.build_agent(&req.config);

        if !req.config.overwrite {
            self.check_not_exists(&agent, &ep, &creds, req, &uri)?;
        }

        let part_size = self.effective_part_size(&req.config);
        let (parts_uploaded, composite) = if req.content_length > part_size {
            let (parts, comp) = self.upload_multipart(&agent, &ep, &creds, req, &uri, part_size)?;
            (parts, Some(comp))
        } else {
            self.upload_single_part(&agent, &ep, &creds, req, &uri, &sha256_b64)?;
            (1, None)
        };

        self.verify_uploaded_object(
            &agent,
            &ep,
            &creds,
            req,
            &uri,
            &sha256_b64,
            composite.as_deref(),
            parts_uploaded,
        )
    }
}

/// Builds a canonical `s3://<bucket>/<key>` URI.
pub fn build_s3_uri(bucket: &str, key: &str) -> String {
    format!("s3://{}/{}", bucket, key.trim_start_matches('/'))
}

/// Computes a deterministic, traversal-safe S3 object key from prefix, optional `strftime` date subdir, and basename.
pub fn build_object_key(
    prefix: Option<&str>,
    key_date_subdir: Option<&str>,
    mtime: DateTime<Utc>,
    file_name: &str,
    scope: &str,
) -> Result<String> {
    if file_name.is_empty()
        || file_name == "."
        || file_name == ".."
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.chars().any(char::is_control)
    {
        return Err(FileCleanerError::ConfigValidation(format!(
            "{scope}: unsafe candidate filename '{file_name}' for S3 object key"
        )));
    }

    let mut segments: Vec<String> = Vec::new();
    if let Some(pref) = prefix {
        push_clean_key_segments(&mut segments, pref, scope, "prefix")?;
    }
    if let Some(fmt) = key_date_subdir {
        if fmt.trim().is_empty() || StrftimeItems::new(fmt).any(|item| matches!(item, Item::Error))
        {
            return Err(FileCleanerError::ConfigValidation(format!(
                "{scope}: invalid key_date_subdir '{fmt}'"
            )));
        }
        let rendered = mtime.format(fmt).to_string();
        push_clean_key_segments(&mut segments, &rendered, scope, "key_date_subdir")?;
    }
    segments.push(file_name.to_string());
    Ok(segments.join("/"))
}

fn push_clean_key_segments(
    out: &mut Vec<String>,
    raw: &str,
    scope: &str,
    field: &str,
) -> Result<()> {
    if raw.contains('\\') || raw.chars().any(char::is_control) {
        return Err(FileCleanerError::ConfigValidation(format!(
            "{scope}: {field} '{raw}' contains unsafe characters"
        )));
    }
    for seg in raw.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            return Err(FileCleanerError::ConfigValidation(format!(
                "{scope}: {field} '{raw}' contains parent traversal '..'"
            )));
        }
        out.push(seg.to_string());
    }
    Ok(())
}

enum RetryableError {
    Transient(FileCleanerError),
    Fatal(FileCleanerError),
}

struct CompletedPart {
    part_number: usize,
    etag: String,
    checksum_sha256_b64: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedEndpoint {
    url_base: String,
    host_header: String,
    canonical_uri: String,
}

fn resolve_endpoint(cfg: &S3Config, key: &str, path: &Path, uri: &str) -> Result<ResolvedEndpoint> {
    let clean_key = key.trim_start_matches('/');
    let enc_bucket = uri_encode(&cfg.bucket, true);
    let enc_key = uri_encode(clean_key, false);

    if let Some(ep) = &cfg.endpoint {
        let ep_clean = ep.trim_end_matches('/');
        let (scheme, rest) = if let Some(r) = ep_clean.strip_prefix("https://") {
            ("https", r)
        } else if let Some(r) = ep_clean.strip_prefix("http://") {
            if !cfg.allow_insecure_http {
                return Err(s3_err(
                    path,
                    uri,
                    "refusing plaintext http:// endpoint with allow_insecure_http = false",
                ));
            }
            ("http", r)
        } else {
            return Err(s3_err(
                path,
                uri,
                format!("unsupported S3 endpoint scheme in '{ep}'"),
            ));
        };

        let (authority, base_path) = match rest.find('/') {
            Some(idx) => (&rest[..idx], rest[idx..].trim_end_matches('/')),
            None => (rest, ""),
        };
        if authority.is_empty() {
            return Err(s3_err(path, uri, format!("empty host in endpoint '{ep}'")));
        }

        if cfg.force_path_style {
            let canonical_uri = format!("{base_path}/{enc_bucket}/{enc_key}");
            Ok(ResolvedEndpoint {
                url_base: format!("{scheme}://{authority}{canonical_uri}"),
                host_header: authority.to_string(),
                canonical_uri,
            })
        } else {
            let host_header = format!("{}.{authority}", cfg.bucket);
            let canonical_uri = format!("{base_path}/{enc_key}");
            Ok(ResolvedEndpoint {
                url_base: format!("{scheme}://{host_header}{canonical_uri}"),
                host_header,
                canonical_uri,
            })
        }
    } else if cfg.force_path_style {
        let host_header = format!("s3.{}.amazonaws.com", cfg.region);
        let canonical_uri = format!("/{enc_bucket}/{enc_key}");
        Ok(ResolvedEndpoint {
            url_base: format!("https://{host_header}{canonical_uri}"),
            host_header,
            canonical_uri,
        })
    } else {
        let host_header = format!("{}.s3.{}.amazonaws.com", cfg.bucket, cfg.region);
        let canonical_uri = format!("/{enc_key}");
        Ok(ResolvedEndpoint {
            url_base: format!("https://{host_header}{canonical_uri}"),
            host_header,
            canonical_uri,
        })
    }
}

struct SignedV4 {
    authorization: String,
    headers: Vec<(String, String)>,
}

#[allow(clippy::too_many_arguments)]
fn sign_v4_request(
    method: &str,
    canonical_uri: &str,
    query_params: &[(&str, &str)],
    extra_headers: &[(&str, &str)],
    payload_sha256_hex: &str,
    creds: &S3Credentials,
    region: &str,
    now: DateTime<Utc>,
) -> SignedV4 {
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date_stamp = now.format("%Y%m%d").to_string();

    let mut headers_map: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in extra_headers {
        headers_map.insert(k.to_ascii_lowercase(), normalize_header_value(v));
    }
    headers_map.insert(
        "x-amz-content-sha256".to_string(),
        payload_sha256_hex.to_string(),
    );
    headers_map.insert("x-amz-date".to_string(), amz_date.clone());
    if let Some(tok) = &creds.session_token {
        headers_map.insert(
            "x-amz-security-token".to_string(),
            normalize_header_value(tok.expose()),
        );
    }

    let mut canonical_headers = String::new();
    let mut signed_names: Vec<&str> = Vec::with_capacity(headers_map.len());
    for (k, v) in &headers_map {
        canonical_headers.push_str(k);
        canonical_headers.push(':');
        canonical_headers.push_str(v);
        canonical_headers.push('\n');
        signed_names.push(k.as_str());
    }
    let signed_headers = signed_names.join(";");
    let canonical_query = canonical_query_string(query_params);

    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_sha256_hex}"
    );
    let canonical_request_hash = sha256_bytes_hex(canonical_request.as_bytes());
    let credential_scope = format!("{date_stamp}/{region}/s3/aws4_request");
    let string_to_sign =
        format!("AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{canonical_request_hash}");

    let signing_key =
        derive_signing_key(creds.secret_access_key.expose(), &date_stamp, region, "s3");
    let signature = to_hex_lower(&hmac_sha256(&signing_key, string_to_sign.as_bytes()));

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}",
        creds.access_key_id
    );

    // Exclude `host` from explicit request headers because `ureq` sets `Host` automatically from URL.
    let headers = headers_map
        .into_iter()
        .filter(|(k, _)| k != "host")
        .collect();

    SignedV4 {
        authorization,
        headers,
    }
}

fn derive_signing_key(secret_key: &str, date_stamp: &str, region: &str, service: &str) -> [u8; 32] {
    let mut k_secret = Vec::with_capacity(4 + secret_key.len());
    k_secret.extend_from_slice(b"AWS4");
    k_secret.extend_from_slice(secret_key.as_bytes());
    let k_date = hmac_sha256(&k_secret, date_stamp.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts keys of any size");
    mac.update(data);
    let result = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

fn uri_encode(input: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        let unreserved = b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'_' | b'.' | b'~')
            || (!encode_slash && b == b'/');
        if unreserved {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(hex_upper_nibble(b >> 4));
            out.push(hex_upper_nibble(b & 0x0F));
        }
    }
    out
}

fn hex_upper_nibble(v: u8) -> char {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    HEX[v as usize] as char
}

fn canonical_query_string(params: &[(&str, &str)]) -> String {
    let mut encoded: Vec<(String, String)> = params
        .iter()
        .map(|(k, v)| (uri_encode(k, true), uri_encode(v, true)))
        .collect();
    encoded.sort();
    encoded
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn normalize_header_value(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn hash_file_range(
    path: &Path,
    offset: u64,
    len: u64,
    full_hasher: &mut Sha256,
) -> Result<([u8; 32], String, String)> {
    let (mut file, _) = open_regular_nofollow_read(path)?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| FileCleanerError::io(path, e))?;

    let mut part_hasher = Sha256::new();
    let mut buf = [0u8; STREAM_BUF_SIZE];
    let mut remaining = len;

    while remaining > 0 {
        let want = (remaining as usize).min(buf.len());
        let n = file
            .read(&mut buf[..want])
            .map_err(|e| FileCleanerError::io(path, e))?;
        if n == 0 {
            return Err(FileCleanerError::IntegrityMismatch {
                path: path.to_path_buf(),
                expected: format!("{len} bytes at offset {offset}"),
                actual: format!("unexpected EOF after {} bytes", len - remaining),
            });
        }
        part_hasher.update(&buf[..n]);
        full_hasher.update(&buf[..n]);
        remaining -= n as u64;
    }

    let digest = part_hasher.finalize();
    let mut raw = [0u8; 32];
    raw.copy_from_slice(&digest);
    let hex = to_hex_lower(&raw);
    let b64 = BASE64_STANDARD.encode(raw);
    Ok((raw, hex, b64))
}

fn compute_composite_multipart_checksum(part_raw_hashes: &[[u8; 32]]) -> String {
    let mut hasher = Sha256::new();
    for raw in part_raw_hashes {
        hasher.update(raw);
    }
    let combined = hasher.finalize();
    format!(
        "{}-{}",
        BASE64_STANDARD.encode(combined),
        part_raw_hashes.len()
    )
}

fn classify_http_status(
    mut resp: ureq::http::Response<ureq::Body>,
    op: &str,
    path: &Path,
    uri: &str,
) -> std::result::Result<ureq::http::Response<ureq::Body>, RetryableError> {
    let status = resp.status().as_u16();
    if (200..300).contains(&status) {
        return Ok(resp);
    }
    let snippet = read_bounded_body_string(&mut resp, MAX_ERROR_BODY_BYTES);
    let msg = if snippet.is_empty() {
        format!("{op} failed with HTTP {status}")
    } else {
        let code = xml_extract_tag(&snippet, "Code");
        let detail = xml_extract_tag(&snippet, "Message");
        match (code, detail) {
            (Some(c), Some(d)) => format!("{op} failed with HTTP {status} ({c}: {d})"),
            (Some(c), None) => format!("{op} failed with HTTP {status} ({c})"),
            _ => format!("{op} failed with HTTP {status}"),
        }
    };
    let err = s3_err(path, uri, msg);
    if (500..=599).contains(&status) {
        Err(RetryableError::Transient(err))
    } else {
        Err(RetryableError::Fatal(err))
    }
}

fn read_bounded_body_string(
    resp: &mut ureq::http::Response<ureq::Body>,
    max_bytes: usize,
) -> String {
    let mut reader = resp.body_mut().as_reader().take(max_bytes as u64);
    let mut buf = Vec::new();
    let _ = reader.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

fn header_str(resp: &ureq::http::Response<ureq::Body>, name: &str) -> Option<String> {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn xml_extract_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)?;
    let val = xml[start..start + end].trim();
    if val.is_empty() {
        None
    } else {
        Some(val.to_string())
    }
}

fn xml_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

fn sha256_bytes_hex(data: &[u8]) -> String {
    to_hex_lower(&Sha256::digest(data))
}

fn to_hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn decode_sha256_hex(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let bytes = hex.as_bytes();
    let mut out = [0u8; 32];
    for i in 0..32 {
        let hi = from_hex_digit(bytes[i * 2])?;
        let lo = from_hex_digit(bytes[i * 2 + 1])?;
        out[i] = (hi << 4) | lo;
    }
    Some(out)
}

fn from_hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn s3_err(path: &Path, uri: &str, reason: impl Into<String>) -> FileCleanerError {
    FileCleanerError::S3Upload {
        path: path.to_path_buf(),
        uri: uri.to_string(),
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::compute_file_sha256;
    use chrono::TimeZone;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;
    use tempfile::TempDir;

    #[derive(Default)]
    struct MockS3State {
        objects: HashMap<String, MockObject>,
        multiparts: HashMap<String, HashMap<usize, (Vec<u8>, String)>>,
        next_upload_id: usize,
        aborted_uploads: Vec<String>,
        fail_put_count: usize,
        fail_head_count: usize,
        fail_part_number: Option<usize>,
        corrupt_head_checksum: Option<String>,
        omit_head_checksum: bool,
        corrupt_head_size: Option<u64>,
        corrupt_get_body: bool,
        complete_embedded_error: bool,
        auth_403: bool,
    }

    #[derive(Clone)]
    struct MockObject {
        data: Vec<u8>,
        checksum_sha256: String,
        etag: String,
    }

    struct MockS3Server {
        addr: SocketAddr,
        state: Arc<Mutex<MockS3State>>,
        shutdown: Arc<AtomicBool>,
        requests: Arc<AtomicUsize>,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl MockS3Server {
        fn start(initial: MockS3State) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let addr = listener.local_addr().unwrap();
            let state = Arc::new(Mutex::new(initial));
            let shutdown = Arc::new(AtomicBool::new(false));
            let requests = Arc::new(AtomicUsize::new(0));

            let st = Arc::clone(&state);
            let sd = Arc::clone(&shutdown);
            let rq = Arc::clone(&requests);
            let handle = thread::spawn(move || {
                while !sd.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                            rq.fetch_add(1, Ordering::SeqCst);
                            handle_http_conn(stream, &st);
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            });

            Self {
                addr,
                state,
                shutdown,
                requests,
                handle: Some(handle),
            }
        }

        fn endpoint(&self) -> String {
            format!("http://{}", self.addr)
        }
    }

    impl Drop for MockS3Server {
        fn drop(&mut self) {
            self.shutdown.store(true, Ordering::SeqCst);
            let _ = TcpStream::connect(self.addr);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }

    fn handle_http_conn(mut stream: TcpStream, state: &Mutex<MockS3State>) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut req_line = String::new();
        if reader.read_line(&mut req_line).unwrap_or(0) == 0 {
            return;
        }
        let mut parts = req_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let target = parts.next().unwrap_or("/").to_string();

        let mut headers = HashMap::<String, String>::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                break;
            }
            if let Some((k, v)) = trimmed.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
            }
        }

        let content_len: usize = headers
            .get("content-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let mut body = vec![0u8; content_len];
        if content_len > 0 && reader.read_exact(&mut body).is_err() {
            return;
        }

        let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
        let key = path.trim_start_matches('/').to_string();
        let mut qmap = HashMap::<String, String>::new();
        for pair in query.split('&').filter(|s| !s.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            qmap.insert(k.to_string(), v.to_string());
        }

        let mut st = state.lock().unwrap();
        if st.auth_403 {
            let xml = "<Error><Code>AccessDenied</Code><Message>Denied</Message></Error>";
            write_http(&mut stream, 403, &[], xml.as_bytes());
            return;
        }

        match method.as_str() {
            "HEAD" => {
                if st.fail_head_count > 0 {
                    st.fail_head_count -= 1;
                    write_http(&mut stream, 503, &[], b"");
                    return;
                }
                if let Some(obj) = st.objects.get(&key).cloned() {
                    let sz = st.corrupt_head_size.unwrap_or(obj.data.len() as u64);
                    let mut hdrs = vec![
                        ("Content-Length".to_string(), sz.to_string()),
                        ("ETag".to_string(), obj.etag.clone()),
                    ];
                    if !st.omit_head_checksum {
                        let ck = st
                            .corrupt_head_checksum
                            .clone()
                            .unwrap_or(obj.checksum_sha256);
                        hdrs.push(("x-amz-checksum-sha256".to_string(), ck));
                    }
                    write_http(&mut stream, 200, &hdrs, b"");
                } else {
                    write_http(&mut stream, 404, &[], b"");
                }
            }
            "PUT" => {
                if let (Some(part_num_s), Some(uid)) =
                    (qmap.get("partNumber"), qmap.get("uploadId"))
                {
                    let part_num: usize = part_num_s.parse().unwrap_or(0);
                    if st.fail_part_number == Some(part_num) {
                        write_http(&mut stream, 500, &[], b"");
                        return;
                    }
                    let part_ck = headers
                        .get("x-amz-checksum-sha256")
                        .cloned()
                        .unwrap_or_default();
                    let etag = format!("\"etag-part-{part_num}\"");
                    st.multiparts
                        .entry(uid.clone())
                        .or_default()
                        .insert(part_num, (body, part_ck));
                    write_http(&mut stream, 200, &[("ETag".into(), etag)], b"");
                    return;
                }
                if st.fail_put_count > 0 {
                    st.fail_put_count -= 1;
                    write_http(&mut stream, 500, &[], b"");
                    return;
                }
                let actual_b64 = BASE64_STANDARD.encode(Sha256::digest(&body));
                let etag = "\"single-etag\"".to_string();
                st.objects.insert(
                    key,
                    MockObject {
                        data: body,
                        checksum_sha256: actual_b64,
                        etag: etag.clone(),
                    },
                );
                write_http(&mut stream, 200, &[("ETag".into(), etag)], b"");
            }
            "POST" => {
                if qmap.contains_key("uploads") {
                    st.next_upload_id += 1;
                    let uid = format!("upload-{}", st.next_upload_id);
                    st.multiparts.insert(uid.clone(), HashMap::new());
                    let xml = format!(
                        "<InitiateMultipartUploadResult><UploadId>{uid}</UploadId></InitiateMultipartUploadResult>"
                    );
                    write_http(&mut stream, 200, &[], xml.as_bytes());
                } else if let Some(uid) = qmap.get("uploadId") {
                    if st.complete_embedded_error {
                        let xml =
                            "<Error><Code>InternalError</Code><Message>retry</Message></Error>";
                        write_http(&mut stream, 200, &[], xml.as_bytes());
                        return;
                    }
                    let parts_map = st.multiparts.remove(uid).unwrap_or_default();
                    let mut nums: Vec<usize> = parts_map.keys().copied().collect();
                    nums.sort();
                    let mut combined = Vec::new();
                    let mut raw_hashes = Vec::new();
                    for n in &nums {
                        let (bytes, _) = &parts_map[n];
                        combined.extend_from_slice(bytes);
                        let mut raw = [0u8; 32];
                        raw.copy_from_slice(&Sha256::digest(bytes));
                        raw_hashes.push(raw);
                    }
                    let composite = compute_composite_multipart_checksum(&raw_hashes);
                    let etag = format!("\"multi-{}\"", nums.len());
                    st.objects.insert(
                        key,
                        MockObject {
                            data: combined,
                            checksum_sha256: composite,
                            etag: etag.clone(),
                        },
                    );
                    let xml = format!(
                        "<CompleteMultipartUploadResult><ETag>{etag}</ETag></CompleteMultipartUploadResult>"
                    );
                    write_http(&mut stream, 200, &[], xml.as_bytes());
                } else {
                    write_http(&mut stream, 400, &[], b"");
                }
            }
            "DELETE" => {
                if let Some(uid) = qmap.get("uploadId") {
                    st.multiparts.remove(uid);
                    st.aborted_uploads.push(uid.clone());
                }
                write_http(&mut stream, 204, &[], b"");
            }
            "GET" => {
                if let Some(obj) = st.objects.get(&key).cloned() {
                    let mut out = obj.data;
                    if st.corrupt_get_body && !out.is_empty() {
                        out[0] ^= 0xFF;
                    }
                    write_http(&mut stream, 200, &[], &out);
                } else {
                    write_http(&mut stream, 404, &[], b"");
                }
            }
            _ => write_http(&mut stream, 405, &[], b""),
        }
    }

    fn write_http(stream: &mut TcpStream, status: u16, headers: &[(String, String)], body: &[u8]) {
        let mut resp = format!("HTTP/1.1 {status} OK\r\nConnection: close\r\n");
        let has_cl = headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-length"));
        if !has_cl {
            resp.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        for (k, v) in headers {
            resp.push_str(&format!("{k}: {v}\r\n"));
        }
        resp.push_str("\r\n");
        let _ = stream.write_all(resp.as_bytes());
        if !body.is_empty() {
            let _ = stream.write_all(body);
        }
        let _ = stream.flush();
    }

    fn test_s3_config(endpoint: String, ak_env: &str, sk_env: &str) -> S3Config {
        S3Config {
            bucket: "pg-archive".to_string(),
            region: "us-east-1".to_string(),
            endpoint: Some(endpoint),
            force_path_style: true,
            allow_insecure_http: true,
            access_key_env: ak_env.to_string(),
            secret_key_env: sk_env.to_string(),
            session_token_env: None,
            timeout: Duration::from_secs(5),
            max_retries: 2,
            ..S3Config::default()
        }
    }

    #[test]
    fn aws_sigv4_matches_official_s3_test_vector_and_redacts_secrets() {
        // Official AWS S3 SigV4 GET Object vector (2013-05-24, us-east-1, examplebucket/test.txt).
        let creds = S3Credentials {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".to_string(),
            secret_access_key: RedactedSecret(
                "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
            ),
            session_token: Some(RedactedSecret("SECRET-SESSION-TOKEN".to_string())),
        };
        let debug_str = format!("{creds:?}");
        assert!(debug_str.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(!debug_str.contains("wJalrXUtnFEMI"));
        assert!(!debug_str.contains("SECRET-SESSION-TOKEN"));
        assert!(debug_str.contains("[REDACTED]"));
        assert_eq!(format!("{}", creds.secret_access_key), "[REDACTED]");

        let creds_no_tok = S3Credentials {
            session_token: None,
            ..creds
        };
        let dt = Utc.with_ymd_and_hms(2013, 5, 24, 0, 0, 0).unwrap();
        let signed = sign_v4_request(
            "GET",
            "/test.txt",
            &[],
            &[
                ("host", "examplebucket.s3.amazonaws.com"),
                ("range", "bytes=0-9"),
            ],
            EMPTY_SHA256,
            &creds_no_tok,
            "us-east-1",
            dt,
        );
        assert_eq!(
            signed.authorization,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );

        // Query encoding, special chars, and endpoint URL resolution (path-style vs virtual-hosted).
        assert_eq!(
            canonical_query_string(&[("uploadId", "a/b c"), ("partNumber", "2")]),
            "partNumber=2&uploadId=a%2Fb%20c"
        );
        let aws_vh = S3Config {
            bucket: "my-bkt".into(),
            region: "eu-west-3".into(),
            ..S3Config::default()
        };
        let ep_vh =
            resolve_endpoint(&aws_vh, "dir/file 1.sql", Path::new("f"), "s3://my-bkt/f").unwrap();
        assert_eq!(ep_vh.host_header, "my-bkt.s3.eu-west-3.amazonaws.com");
        assert_eq!(ep_vh.canonical_uri, "/dir/file%201.sql");

        let aws_ps = S3Config {
            force_path_style: true,
            ..aws_vh
        };
        let ep_ps = resolve_endpoint(&aws_ps, "a/b.sql", Path::new("f"), "s3://my-bkt/f").unwrap();
        assert_eq!(ep_ps.host_header, "s3.eu-west-3.amazonaws.com");
        assert_eq!(ep_ps.canonical_uri, "/my-bkt/a/b.sql");

        let custom_vh = S3Config {
            bucket: "bkt".into(),
            region: "us-east-1".into(),
            endpoint: Some("https://s3.internal:9000/base".into()),
            force_path_style: false,
            ..S3Config::default()
        };
        let ep_cvh = resolve_endpoint(&custom_vh, "k.gz", Path::new("f"), "s3://bkt/k.gz").unwrap();
        assert_eq!(ep_cvh.host_header, "bkt.s3.internal:9000");
        assert_eq!(ep_cvh.canonical_uri, "/base/k.gz");
    }

    #[test]
    fn build_object_key_handles_prefix_date_and_rejects_traversal() {
        let dt = Utc.with_ymd_and_hms(2026, 11, 22, 10, 0, 0).unwrap();
        let key = build_object_key(
            Some("/pg/wal/"),
            Some("%Y/%m/%d"),
            dt,
            "000000010000000000000001.lz4",
            "rule",
        )
        .unwrap();
        assert_eq!(key, "pg/wal/2026/11/22/000000010000000000000001.lz4");
        assert_eq!(build_s3_uri("bkt", &key), format!("s3://bkt/{key}"));

        assert!(build_object_key(Some("a/../b"), None, dt, "ok.sql", "r").is_err());
        assert!(build_object_key(None, Some("%Q"), dt, "ok.sql", "r").is_err());
        assert!(build_object_key(None, None, dt, "../escape.sql", "r").is_err());
    }

    #[test]
    fn loopback_s3_single_part_multipart_readback_retries_and_fail_closed_errors() {
        let tmp = TempDir::new().unwrap();
        let sample = tmp.path().join("wal_0001.lz4");
        let payload: Vec<u8> = (0..10_000).map(|i| (i % 251) as u8).collect();
        std::fs::write(&sample, &payload).unwrap();
        let (sha, len) = compute_file_sha256(&sample).unwrap();

        let ak_env = "FC_TEST_S3_ACCESS_KEY";
        let sk_env = "FC_TEST_S3_SECRET_KEY";
        std::env::set_var(ak_env, "test-ak");
        std::env::set_var(sk_env, "test-sk");

        // 1. Single-part PUT + HEAD x-amz-checksum-sha256 + 5xx retry recovery.
        let srv = MockS3Server::start(MockS3State {
            fail_put_count: 1,
            fail_head_count: 1,
            ..MockS3State::default()
        });
        let mut cfg = test_s3_config(srv.endpoint(), ak_env, sk_env);
        cfg.storage_class = Some("STANDARD_IA".to_string());
        let uploader = HttpS3Uploader::with_base_backoff(Duration::from_millis(1));
        assert!(format!("{uploader:?}").contains("HttpS3Uploader"));

        let req = S3UploadRequest {
            local_path: sample.clone(),
            key: "pg/wal_0001.lz4".to_string(),
            content_length: len,
            sha256_hex: sha.clone(),
            config: cfg.clone(),
        };
        let receipt = uploader.upload_and_verify(&req).unwrap();
        assert_eq!(receipt.uri, "s3://pg-archive/pg/wal_0001.lz4");
        assert_eq!(receipt.verified_sha256, sha);
        assert_eq!(receipt.verified_size, len);
        assert_eq!(receipt.parts_uploaded, 1);
        assert_eq!(receipt.verify_mode, S3VerifyMode::ChecksumSha256);

        // 2. Non-overwrite rejection returns AlreadyExists when object already exists.
        let dup_err = uploader.upload_and_verify(&req).unwrap_err();
        assert!(matches!(
            dup_err,
            FileCleanerError::Io { ref source, .. } if source.kind() == ErrorKind::AlreadyExists
        ));

        // 3. Overwrite + ReadbackSha256 mode succeeds and detects corrupt GetObject readback.
        let mut readback_req = req.clone();
        readback_req.config.overwrite = true;
        readback_req.config.verify_mode = S3VerifyMode::ReadbackSha256;
        let rb_receipt = uploader.upload_and_verify(&readback_req).unwrap();
        assert_eq!(rb_receipt.verify_mode, S3VerifyMode::ReadbackSha256);

        srv.state.lock().unwrap().corrupt_get_body = true;
        assert!(matches!(
            uploader.upload_and_verify(&readback_req),
            Err(FileCleanerError::IntegrityMismatch { .. })
        ));
        srv.state.lock().unwrap().corrupt_get_body = false;

        // 4. HeadObject missing checksum, wrong checksum, and wrong Content-Length -> IntegrityMismatch.
        let mut ow_req = req.clone();
        ow_req.config.overwrite = true;
        srv.state.lock().unwrap().omit_head_checksum = true;
        assert!(matches!(
            uploader.upload_and_verify(&ow_req),
            Err(FileCleanerError::IntegrityMismatch { .. })
        ));
        srv.state.lock().unwrap().omit_head_checksum = false;

        srv.state.lock().unwrap().corrupt_head_checksum = Some("badchecksum==".into());
        assert!(matches!(
            uploader.upload_and_verify(&ow_req),
            Err(FileCleanerError::IntegrityMismatch { .. })
        ));
        srv.state.lock().unwrap().corrupt_head_checksum = None;

        srv.state.lock().unwrap().corrupt_head_size = Some(len + 1);
        assert!(matches!(
            uploader.upload_and_verify(&ow_req),
            Err(FileCleanerError::IntegrityMismatch { .. })
        ));
        srv.state.lock().unwrap().corrupt_head_size = None;

        // 5. Multipart upload success (3 parts) with composite S3 SHA-256 verification.
        let mp_uploader = uploader.clone().with_multipart_override(4_000);
        let mut mp_req = req.clone();
        mp_req.key = "pg/multi.lz4".to_string();
        let mp_receipt = mp_uploader.upload_and_verify(&mp_req).unwrap();
        assert_eq!(mp_receipt.parts_uploaded, 3);
        assert_eq!(mp_receipt.verified_size, len);

        // 6. Multipart upload aborts cleanly when a part fails after exhausting retries.
        srv.state.lock().unwrap().fail_part_number = Some(2);
        let mut mp_fail_req = mp_req.clone();
        mp_fail_req.key = "pg/multi_fail.lz4".to_string();
        assert!(matches!(
            mp_uploader.upload_and_verify(&mp_fail_req),
            Err(FileCleanerError::S3Upload { .. })
        ));
        assert!(!srv.state.lock().unwrap().aborted_uploads.is_empty());
        srv.state.lock().unwrap().fail_part_number = None;

        // 7. Embedded <Error> inside HTTP 200 CompleteMultipartUpload triggers abort & error.
        srv.state.lock().unwrap().complete_embedded_error = true;
        let mut mp_emb_req = mp_req.clone();
        mp_emb_req.key = "pg/multi_emb.lz4".to_string();
        assert!(matches!(
            mp_uploader.upload_and_verify(&mp_emb_req),
            Err(FileCleanerError::S3Upload { .. })
        ));
        srv.state.lock().unwrap().complete_embedded_error = false;

        // 8. 403 AccessDenied fails fast without retrying.
        srv.state.lock().unwrap().auth_403 = true;
        let before = srv.requests.load(Ordering::SeqCst);
        assert!(matches!(
            uploader.upload_and_verify(&ow_req),
            Err(FileCleanerError::S3Upload { .. })
        ));
        assert_eq!(srv.requests.load(Ordering::SeqCst) - before, 1);

        // 9. Missing env var credentials fail cleanly without hitting the network.
        let mut missing_env = req.clone();
        missing_env.config.access_key_env = "FC_DEFINITELY_UNSET_AK".to_string();
        assert!(matches!(
            uploader.upload_and_verify(&missing_env),
            Err(FileCleanerError::S3Upload { .. })
        ));
    }
}
