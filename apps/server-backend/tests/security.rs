//! What a hostile client can send the server.
//!
//! Accepting `envelope=tar` means the server extracts an archive it did not
//! create, which is the dangerous direction. `apps/core/tests/security.rs`
//! already proves the engine's extractor holds; these tests prove the
//! **server** is wired to it correctly, that a job fails instead of producing
//! something, and above all that nothing lands outside the job's staging
//! directory. That last part is the claim `docs/threat_model.md` makes.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use tar::{Builder, Header};
use tower::util::ServiceExt;

use collapse_server_backend::{build_app, DEFAULT_MAX_UPLOAD_MB};

fn app() -> (Router, tempfile::TempDir) {
    let storage = tempfile::TempDir::new().unwrap();
    let router =
        build_app(storage.path().to_path_buf(), DEFAULT_MAX_UPLOAD_MB).expect("the app builds");
    (router, storage)
}

async fn post_envelope(router: &Router, name: &str, tar: &[u8]) -> Response {
    let request = Request::builder()
        .method(Method::POST)
        .uri(format!("/compress?name={name}&envelope=tar"))
        .body(Body::from(tar.to_vec()))
        .unwrap();
    router.clone().oneshot(request).await.unwrap()
}

async fn json_of(response: Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

/// Upload a tar envelope and wait for the job to settle.
async fn settle(router: &Router, name: &str, tar: &[u8]) -> serde_json::Value {
    let accepted = post_envelope(router, name, tar).await;
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    let job_id = json_of(accepted).await["job_id"]
        .as_str()
        .unwrap()
        .to_string();

    // About thirty seconds: these payloads settle in milliseconds anywhere,
    // but a loaded CI runner can stall a worker, and a timeout here would read
    // as a security failure rather than a slow machine.
    for _ in 0..3000 {
        let request = Request::builder()
            .uri(format!("/jobs/{job_id}"))
            .body(Body::empty())
            .unwrap();
        let job = json_of(router.clone().oneshot(request).await.unwrap()).await;
        match job["status"].as_str().unwrap() {
            "queued" | "compressing" => {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await
            }
            _ => return job,
        }
    }
    panic!("the job never settled");
}

/// A tar carrying one entry under a name the writer would normally refuse.
/// The raw header bytes are written directly, the same smuggling technique
/// `apps/core/tests/security.rs` uses.
fn tar_with_smuggled_name(entry_name: &str) -> Vec<u8> {
    let mut builder = Builder::new(Vec::new());
    let content = b"pwned";
    let mut header = Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(0o644);
    let name = entry_name.as_bytes();
    header.as_old_mut().name[..name.len()].copy_from_slice(name);
    header.set_cksum();
    builder.append(&header, &content[..]).unwrap();
    builder.into_inner().unwrap()
}

/// Everything in the staging area, relative to it, so a test can assert on
/// exactly what the server wrote.
///
/// The entries carry the platform's separator, so they are only ever searched
/// for a substring or joined back onto the base here; do not compare one
/// against a literal path without normalizing it first.
fn staged(storage: &tempfile::TempDir) -> Vec<String> {
    fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            out.push(
                path.strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
            if path.is_dir() {
                walk(&path, base, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(storage.path(), storage.path(), &mut out);
    out.sort();
    out
}

// ---------------------------------------------------------------------------

/// The classic one: an entry that walks up out of the extraction directory.
#[tokio::test]
async fn a_traversing_entry_never_escapes_the_staging_area() {
    let (router, storage) = app();
    let outside = storage.path().parent().unwrap().join("escaped.txt");

    let job = settle(&router, "photos", &tar_with_smuggled_name("../escaped.txt")).await;

    assert_eq!(job["status"], "failed", "job was {job}");
    assert!(
        !outside.exists(),
        "the entry escaped to {}",
        outside.display()
    );
    assert!(
        !staged(&storage).iter().any(|p| p.contains("escaped")),
        "staged: {:?}",
        staged(&storage)
    );
}

/// The entry name is the attacker's choice, not the server's platform, so it
/// stays a POSIX absolute path on every host. It is still meaningful on
/// Windows: `/tmp/escaped.txt` there is rooted on the current drive, so a naive
/// extractor that passed the name straight to the filesystem would create
/// `C:\tmp\escaped.txt`, which is exactly what this asserts did not happen.
#[tokio::test]
async fn an_absolute_entry_never_escapes_the_staging_area() {
    let (router, _storage) = app();

    let job = settle(
        &router,
        "photos",
        &tar_with_smuggled_name("/tmp/escaped.txt"),
    )
    .await;

    // Either the extractor refuses it or it lands inside; what must never
    // happen is a write to the absolute path.
    assert!(
        !std::path::Path::new("/tmp/escaped.txt").exists(),
        "the entry was written to an absolute path"
    );
    assert!(
        job["status"] == "failed" || job["status"] == "completed",
        "job was {job}"
    );
}

/// A symlink entry must not be materialized, so nothing can be written
/// through it afterwards.
#[tokio::test]
async fn a_symlink_entry_is_not_materialized() {
    let (router, storage) = app();

    let mut builder = Builder::new(Vec::new());
    let mut root = Header::new_gnu();
    root.set_entry_type(tar::EntryType::Directory);
    root.set_size(0);
    root.set_mode(0o755);
    builder.append_data(&mut root, "photos/", &b""[..]).unwrap();
    let mut link = Header::new_gnu();
    link.set_size(0);
    link.set_mode(0o777);
    builder
        .append_link(&mut link, "photos/sneak", "/etc/passwd")
        .unwrap();
    let tar = builder.into_inner().unwrap();

    settle(&router, "photos", &tar).await;

    let planted = storage.path().join("");
    for entry in staged(&storage) {
        let path = planted.join(&entry);
        assert!(
            !path.is_symlink(),
            "a symlink was materialized at {}",
            path.display()
        );
    }
}

/// A tar whose only top-level entry is a file, not a directory, must be
/// refused rather than compressed into something the name does not describe.
#[tokio::test]
async fn an_envelope_that_is_not_a_directory_is_refused() {
    let (router, _storage) = app();

    let mut builder = Builder::new(Vec::new());
    let content = b"just a file";
    let mut header = Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(0o644);
    builder
        .append_data(&mut header, "photos", &content[..])
        .unwrap();
    let tar = builder.into_inner().unwrap();

    let job = settle(&router, "photos", &tar).await;

    assert_eq!(job["status"], "failed", "job was {job}");
    assert!(
        job["error_message"].as_str().unwrap().contains("directory"),
        "unhelpful message: {}",
        job["error_message"]
    );
}

/// An empty tar has no directory to compress.
#[tokio::test]
async fn an_empty_envelope_is_refused() {
    let (router, _storage) = app();
    let builder = Builder::new(Vec::new());
    let tar = builder.into_inner().unwrap();

    let job = settle(&router, "photos", &tar).await;
    assert_eq!(job["status"], "failed", "job was {job}");
}
