//! TEMPORARY probe - delete after running.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;

use collapse_core::Algorithm;
use collapse_remote::{compress_path, RemoteError};

/// Raw HTTP stub: answers each request with the next canned response.
fn raw_serve(responses: Vec<Vec<u8>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for (i, mut stream) in listener.incoming().flatten().enumerate() {
            // Read the request head (and any body) loosely: just grab what's there.
            let mut buf = [0u8; 65536];
            let n = stream.read(&mut buf).unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n.min(120)]).to_string();
            eprintln!("STUB[{i}] <- {}", head.lines().next().unwrap_or(""));
            match responses.get(i) {
                Some(r) => {
                    let _ = stream.write_all(r);
                    let _ = stream.flush();
                    // Graceful FIN, and give the client time to drain before
                    // the socket is dropped (a drop with unread data sends RST).
                    let _ = stream.shutdown(std::net::Shutdown::Write);
                    std::thread::sleep(std::time::Duration::from_millis(300));
                }
                None => break,
            }
            drop(stream);
        }
    });
    format!("http://{addr}")
}

fn source() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("notes.txt");
    std::fs::write(&p, b"x").unwrap();
    (dir, p)
}

fn accepted(id: &str) -> Vec<u8> {
    let body = format!("{{\"job_id\":\"{id}\",\"status\":\"queued\"}}");
    format!(
        "HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn completed(id: &str) -> Vec<u8> {
    let body = format!("{{\"job_id\":\"{id}\",\"status\":\"completed\"}}");
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

#[test]
fn probe_truncated_download_with_content_length() {
    // Content-Length says 5000, body is 10 bytes, then the socket closes.
    let mut short = b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 5000\r\nConnection: close\r\n\r\n".to_vec();
    short.extend_from_slice(b"PK\x03\x04short");

    let server = raw_serve(vec![accepted("j1"), completed("j1"), short]);
    let (_d, src) = source();
    let err = compress_path(&server, &src, Algorithm::Zip, 3).expect_err("truncated");
    println!("TRUNC-CL variant = {err:?}");
    println!("TRUNC-CL display = {err}");
}

#[test]
fn probe_truncated_download_close_delimited() {
    // No Content-Length at all: ureq reads until the socket closes, so a
    // truncated transfer is indistinguishable from a complete one.
    let mut short =
        b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n"
            .to_vec();
    short.extend_from_slice(b"PK\x03\x04truncated-garbage");

    let server = raw_serve(vec![accepted("j2"), completed("j2"), short]);
    let (_d, src) = source();
    let out = compress_path(&server, &src, Algorithm::Zip, 3);
    println!("TRUNC-CLOSE result = {:?}", out.as_ref().map(|b| b.len()));
}

#[test]
fn probe_204_download() {
    let server = raw_serve(vec![
        accepted("j3"),
        completed("j3"),
        b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_vec(),
    ]);
    let (_d, src) = source();
    let out = compress_path(&server, &src, Algorithm::Zip, 3);
    println!("204 result = {:?}", out.as_ref().map(|b| b.len()));
}

#[test]
fn probe_html_error_page_on_download() {
    let body = "<html><body>502 Bad Gateway</body></html>";
    let resp = format!(
        "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let server = raw_serve(vec![accepted("j4"), completed("j4"), resp.into_bytes()]);
    let (_d, src) = source();
    let err = compress_path(&server, &src, Algorithm::Zip, 3).expect_err("502");
    println!("DL-502 variant = {err:?}");
    println!("DL-502 display = {err}");
}

#[test]
fn probe_upload_dies_midway() {
    // The stub accepts the connection and immediately drops it.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            drop(stream); // RST / FIN before any response
        }
    });
    let server = format!("http://{addr}");
    let (_d, src) = source();
    let err = compress_path(&server, &src, Algorithm::Zip, 3).expect_err("dead connection");
    println!("DEAD-CONN variant = {err:?}");
    println!("DEAD-CONN display = {err}");
}

#[test]
fn probe_scheme_less_server_url() {
    let (_d, src) = source();
    for url in ["localhost:8000", "", "not a url", "ftp://host/x"] {
        let err = compress_path(url, &src, Algorithm::Zip, 3).expect_err("bad url");
        println!("BADURL {url:?} => {err:?} / display={err}");
    }
}

#[test]
fn probe_root_dir_source() {
    // A directory whose file_name() is None.
    let err = compress_path("http://127.0.0.1:9", Path::new("/"), Algorithm::Zip, 3)
        .expect_err("root");
    println!("ROOT = {err:?}");
}

fn real_server(max_mb: usize) -> (String, std::path::PathBuf) {
    let storage = tempfile::TempDir::new().unwrap();
    let path = storage.path().to_path_buf();
    let app_path = path.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _keep = storage;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(l.local_addr().unwrap()).unwrap();
            axum::serve(l, collapse_server_backend::build_app(app_path, max_mb).unwrap())
                .await
                .unwrap();
        });
    });
    (format!("http://{}", rx.recv().unwrap()), path)
}

#[test]
fn probe_over_the_upload_cap() {
    let (server, _s) = real_server(1);
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("big.bin");
    std::fs::write(&src, vec![7u8; 3 * 1024 * 1024]).unwrap();
    let err = compress_path(&server, &src, Algorithm::Zip, 3).expect_err("over cap");
    println!("CAP variant = {err:?}");
    println!("CAP display = {err}");
}

#[test]
fn probe_awkward_file_names() {
    let (server, _s) = real_server(64);
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "my report & notes.txt",
        "café.txt",
        "100%done.txt",
        "a+b=c.txt",
        "weird#hash.txt",
    ] {
        let src = dir.path().join(name);
        std::fs::write(&src, b"payload").unwrap();
        match compress_path(&server, &src, Algorithm::Zip, 3) {
            Ok(bytes) => {
                let out = dir.path().join(format!("{}.zip", name.len()));
                std::fs::write(&out, &bytes).unwrap();
                let into = dir.path().join(format!("x{}", name.len()));
                println!(
                    "NAME {name:?} => ok, entries {:?}",
                    collapse_core::extract(&out, &into)
                );
            }
            Err(e) => println!("NAME {name:?} => ERR {e:?}"),
        }
    }
}

#[test]
fn probe_leak_after_failed_download() {
    // Real server, then delete the job out from under the client? Simpler:
    // check whether a failed run leaves the job dir behind, using a stub that
    // truncates the download.
    let mut short = b"HTTP/1.1 200 OK\r\nContent-Length: 5000\r\nConnection: close\r\n\r\n".to_vec();
    short.extend_from_slice(b"short");
    let server = raw_serve(vec![accepted("j9"), completed("j9"), short]);
    let (_d, src) = source();
    let _ = compress_path(&server, &src, Algorithm::Zip, 3);
    // STUB log above shows whether a DELETE was ever issued.
}

#[test]
fn probe_health_404() {
    let server = raw_serve(vec![
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    ]);
    let err = collapse_remote::check_health(&server).expect_err("404");
    println!("HEALTH404 = {err:?} / display={err}");
}

#[test]
fn probe_empty_dir_and_symlinked_dir() {
    let (server, _s) = real_server(64);
    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    println!("EMPTYDIR = {:?}", compress_path(&server, &empty, Algorithm::Zip, 3).map(|b| b.len()));

    #[cfg(unix)]
    {
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("a.txt"), b"hi").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        match compress_path(&server, &link, Algorithm::Zip, 3) {
            Ok(b) => {
                let out = dir.path().join("l.zip");
                std::fs::write(&out, b).unwrap();
                println!(
                    "SYMLINKDIR = ok, entries {:?}",
                    collapse_core::extract(&out, &dir.path().join("lo"))
                );
            }
            Err(e) => println!("SYMLINKDIR = ERR {e:?}"),
        }
    }
}

#[test]
fn probe_non_json_202_body() {
    let body = "<html>hi</html>";
    let resp = format!(
        "HTTP/1.1 202 Accepted\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let server = raw_serve(vec![resp.into_bytes()]);
    let (_d, src) = source();
    let err = compress_path(&server, &src, Algorithm::Zip, 3).expect_err("not json");
    println!("NONJSON variant = {err:?}");
    println!("NONJSON display = {err}");
}

#[test]
fn probe_server_that_never_answers() {
    // Accepts the connection, reads the request, and never writes anything.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for mut s in listener.incoming().flatten() {
            let mut b = [0u8; 65536];
            let _ = s.read(&mut b);
            held.push(s); // never respond, never close
        }
    });
    let server = format!("http://{addr}");
    let (_d, src) = source();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let r = compress_path(&server, &src, Algorithm::Zip, 3);
        let _ = tx.send(format!("{r:?}"));
    });
    match rx.recv_timeout(std::time::Duration::from_secs(6)) {
        Ok(r) => println!("NEVER-ANSWERS returned: {r}"),
        Err(_) => println!("NEVER-ANSWERS: still blocked after 6s (no read timeout)"),
    }
}

#[test]
fn probe_poll_loop_connections() {
    // queued, queued, queued, completed, download, delete
    let mut resps = vec![accepted("jp")];
    for _ in 0..3 {
        let b = "{\"job_id\":\"jp\",\"status\":\"compressing\"}";
        resps.push(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}", b.len()).into_bytes());
    }
    resps.push(completed("jp"));
    resps.push(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nZIP".to_vec());
    resps.push(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_vec());
    let server = raw_serve(resps);
    let (_d, src) = source();
    println!("POLL result = {:?}", compress_path(&server, &src, Algorithm::Zip, 3));
}
