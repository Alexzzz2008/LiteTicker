use super::*;
use serde_json::json;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Mutex, atomic::AtomicBool},
    thread,
    time::Instant,
};
fn put16(data: &mut [u8], at: usize, value: u16) {
    data[at..at + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn utf16(value: &str) -> Vec<u8> {
    value
        .encode_utf16()
        .chain(Some(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}
fn block(key: &str, kind: u16, value: &[u8], children: Vec<Vec<u8>>) -> Vec<u8> {
    let mut result = vec![0; 6];
    result.extend(utf16(key));
    while result.len() % 4 != 0 {
        result.push(0);
    }
    result.extend(value);
    while result.len() % 4 != 0 {
        result.push(0);
    }
    for child in children {
        result.extend(child);
        while result.len() % 4 != 0 {
            result.push(0);
        }
    }
    let len = result.len() as u16;
    put16(&mut result, 0, len);
    put16(
        &mut result,
        2,
        if kind == 1 {
            (value.len() / 2) as u16
        } else {
            value.len() as u16
        },
    );
    put16(&mut result, 4, kind);
    result
}
fn executable(name: &str, product: &str, managed: bool) -> Vec<u8> {
    let version = version(name).unwrap();
    let mut fixed = vec![0; 52];
    put32(&mut fixed, 0, 0xFEEF04BD);
    put32(&mut fixed, 8, (version.0 as u32) << 16 | version.1 as u32);
    put32(&mut fixed, 12, (version.2 as u32) << 16);
    let resource = block(
        "VS_VERSION_INFO",
        0,
        &fixed,
        vec![block(
            "StringFileInfo",
            1,
            &[],
            vec![block(
                "040904b0",
                1,
                &[],
                vec![
                    block("ProductName", 1, &utf16(product), vec![]),
                    block("OriginalFilename", 1, &utf16(ASSET), vec![]),
                    block("ProductVersion", 1, &utf16(name), vec![]),
                ],
            )],
        )],
    );
    let mut res = vec![0; 88];
    put16(&mut res, 14, 1);
    put32(&mut res, 16, 16);
    put32(&mut res, 20, 0x80000018);
    put16(&mut res, 24 + 14, 1);
    put32(&mut res, 40, 1);
    put32(&mut res, 44, 0x80000030);
    put16(&mut res, 48 + 14, 1);
    put32(&mut res, 64, 1033);
    put32(&mut res, 68, 72);
    put32(&mut res, 72, 0x1000 + 88);
    put32(&mut res, 76, resource.len() as u32);
    res.extend(resource);
    let mut file = vec![0; 512 + ((res.len() + 511) / 512) * 512];
    file[..2].copy_from_slice(b"MZ");
    put32(&mut file, 0x3c, 0x80);
    file[0x80..0x84].copy_from_slice(b"PE\0\0");
    put16(&mut file, 0x84, 0x8664);
    put16(&mut file, 0x86, 1);
    put16(&mut file, 0x94, 240);
    put16(&mut file, 0x96, 2);
    put16(&mut file, 0x98, 0x20b);
    put32(&mut file, 0x98 + 108, 16);
    put32(&mut file, 0x98 + 128, 0x1000);
    put32(&mut file, 0x98 + 132, res.len() as u32);
    if managed {
        put32(&mut file, 0x98 + 224, 0x3000);
        put32(&mut file, 0x98 + 228, 72);
    }
    put32(&mut file, 0x188 + 12, 0x1000);
    let raw_size = file.len() - 512;
    put32(&mut file, 0x188 + 16, raw_size as u32);
    put32(&mut file, 0x188 + 20, 512);
    file[512..512 + res.len()].copy_from_slice(&res);
    file
}
fn release(name: &str, bytes: &[u8]) -> Release {
    let tag = format!("native-v{name}");
    Release {
        version: name.into(),
        sha256: hex(&Sha256::digest(bytes)),
        size: bytes.len() as u64,
        notes: "原生预览测试".into(),
        download_url: Url::parse(&format!(
            "https://github.com/Alexzzz2008/LiteTicker/releases/download/{tag}/{ASSET}"
        ))
        .unwrap(),
        page_url: Url::parse(&format!(
            "https://github.com/Alexzzz2008/LiteTicker/releases/tag/{tag}"
        ))
        .unwrap(),
    }
}
fn metadata(release: &Release) -> serde_json::Value {
    json!({"tag_name":release_tag(release),"html_url":release.page_url.as_str(),"draft":false,"prerelease":true,"body":release.notes,"assets":[{"name":ASSET,"state":"uploaded","size":release.size,"digest":format!("sha256:{}",release.sha256),"browser_download_url":release.download_url.as_str()}]})
}
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let mut dir = OwnedDirectory::new_in(&std::env::temp_dir()).unwrap();
        dir.keep = true;
        Self(dir.path.clone())
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
struct Reply {
    status: u16,
    body: Vec<u8>,
    location: Option<String>,
    delay: Duration,
}
impl Reply {
    fn body(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            body,
            location: None,
            delay: Duration::ZERO,
        }
    }
}
struct Server {
    origin: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(vec![]));
        let logged = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let handle = thread::spawn(move || {
            for reply in replies {
                let deadline = Instant::now() + Duration::from_secs(15);
                let mut stream = loop {
                    if stopped.load(Ordering::Relaxed) || Instant::now() > deadline {
                        return;
                    }
                    match listener.accept() {
                        Ok((s, _)) => break s,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2))
                        }
                        Err(e) => panic!("{e}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buf = [0; 1024];
                while !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => bytes.extend_from_slice(&buf[..n]),
                    }
                    if bytes.len() > 16384 {
                        break;
                    }
                }
                logged
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&bytes).into_owned());
                thread::sleep(reply.delay);
                let location = reply
                    .location
                    .map(|s| format!("Location: {s}\r\n"))
                    .unwrap_or_default();
                let _=stream.write_all(format!("HTTP/1.1 {} Fixture\r\nContent-Length: {}\r\n{location}Connection: close\r\n\r\n",reply.status,reply.body.len()).as_bytes());
                let _ = stream.write_all(&reply.body);
            }
        });
        Self {
            origin,
            requests,
            stop,
            handle: Some(handle),
        }
    }
    fn client(&self, temp: &Temp) -> UpdateClient {
        let mut client = UpdateClient::new().unwrap();
        client.origin = Some(self.origin.clone());
        client.temporary_root = Some(temp.0.clone());
        client
    }
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.take().unwrap().join().unwrap();
    }
}

#[test]
fn only_independent_prerelease_tags_and_official_assets_are_accepted() {
    let bytes = executable("1.0.6-preview.2", PRODUCT, false);
    let r = release("1.0.6-preview.2", &bytes);
    assert!(parse_release(&metadata(&r)).is_ok());
    for tag in [
        "v1.0.6",
        "native-v1.0.6",
        "native-v01.0.6-preview.2",
        "native-v1.0.6-preview.0",
    ] {
        let mut m = metadata(&r);
        m["tag_name"] = json!(tag);
        assert!(parse_release(&m).is_err());
    }
    for (key, value) in [("prerelease", json!(false)), ("draft", json!(true))] {
        let mut m = metadata(&r);
        m[key] = value;
        assert!(parse_release(&m).is_err());
    }
    for (field, value) in [
        ("size", json!(0)),
        ("size", json!(LIMIT + 1)),
        ("size", json!(1.5)),
        ("digest", json!("sha256:no")),
        ("name", json!("LiteTicker.exe")),
        ("state", json!("new")),
        (
            "browser_download_url",
            json!("https://evil.example/LiteTicker-preview.exe"),
        ),
    ] {
        let mut m = metadata(&r);
        m["assets"][0][field] = value;
        assert!(parse_release(&m).is_err());
    }
    let mut m = metadata(&r);
    let asset = m["assets"][0].clone();
    m["assets"].as_array_mut().unwrap().push(asset);
    assert!(parse_release(&m).is_err());
}
#[test]
fn native_identity_refuses_managed_wrong_product_corrupt_headers_and_version() {
    let bytes = executable("1.0.6-preview.2", PRODUCT, false);
    assert_eq!(
        native_identity(&bytes).unwrap().version,
        version("1.0.6-preview.2").unwrap()
    );
    assert!(native_identity(&executable("1.0.6-preview.2", PRODUCT, true)).is_err());
    assert!(native_identity(&executable("1.0.6-preview.2", "LiteTicker", false)).is_err());
    for n in [0, 1, 63, 128, 255, 511] {
        assert!(native_identity(&bytes[..n]).is_err());
    }
    let mut bad = bytes.clone();
    put16(&mut bad, 0x84, 0x14c);
    assert!(native_identity(&bad).is_err());
    let mut bad = bytes.clone();
    put16(&mut bad, 0x98, 0x10b);
    assert!(native_identity(&bad).is_err());
    let mut bad = bytes.clone();
    put32(&mut bad, 0x98 + 128, u32::MAX);
    assert!(native_identity(&bad).is_err());
}
#[test]
fn download_hops_are_bounded_to_fixed_proxy_or_github_cdn() {
    let r = release(
        "1.0.6-preview.2",
        &executable("1.0.6-preview.2", PRODUCT, false),
    );
    for uri in [
        r.download_url.as_str(),
        "https://release-assets.githubusercontent.com/asset?token=temporary",
        "https://objects.githubusercontent.com/asset",
        "https://github-releases.githubusercontent.com/asset",
    ] {
        assert!(validate_hop(&Url::parse(uri).unwrap(), &r).is_ok());
    }
    assert!(
        validate_hop(
            &Url::parse(&format!("https://gh-proxy.com/{}", r.download_url)).unwrap(),
            &r
        )
        .is_ok()
    );
    for uri in [
        "http://github.com/a",
        "https://user@github.com/a",
        "https://github.com:444/a",
        "https://evil.example/a",
        "https://gh-proxy.com/https://evil.example/a",
        "https://github.com/Alexzzz2008/LiteTicker/releases/download/v1.0.5/LiteTicker.exe",
    ] {
        assert!(validate_hop(&Url::parse(uri).unwrap(), &r).is_err());
    }
}
#[tokio::test]
async fn check_new_equal_unavailable_and_failures_are_distinct() {
    let temp = Temp::new();
    let bytes = executable("1.0.6-preview.2", PRODUCT, false);
    let newer = release("1.0.6-preview.2", &bytes);
    let current = release(crate::VERSION, &executable(crate::VERSION, PRODUCT, false));
    let old = release(
        "1.0.5-preview.1",
        &executable("1.0.5-preview.1", PRODUCT, false),
    );
    let server = Server::new(vec![
        Reply::body(
            serde_json::to_vec(&json!([
                metadata(&old),
                metadata(&newer),
                metadata(&current)
            ]))
            .unwrap(),
        ),
        Reply::body(serde_json::to_vec(&json!([metadata(&current)])).unwrap()),
        Reply::body(br#"[{"tag_name":"v1.0.5"}]"#.to_vec()),
        Reply {
            status: 404,
            ..Reply::body(vec![])
        },
        Reply {
            status: 502,
            ..Reply::body(vec![])
        },
        Reply::body(b"{broken".to_vec()),
    ]);
    let c = server.client(&temp);
    assert_eq!(
        c.check(CancellationToken::new())
            .await
            .unwrap()
            .unwrap()
            .version,
        "1.0.6-preview.2"
    );
    assert!(c.check(CancellationToken::new()).await.unwrap().is_none());
    for _ in 0..2 {
        assert!(
            c.check(CancellationToken::new())
                .await
                .unwrap_err()
                .downcast_ref::<ChannelUnavailable>()
                .is_some()
        );
    }
    for _ in 0..2 {
        assert!(c.check(CancellationToken::new()).await.is_err());
    }
    assert_eq!(server.count(), 6);
}
#[tokio::test]
async fn verified_download_reports_progress_and_owns_cleanup() {
    let temp = Temp::new();
    let bytes = executable("1.0.6-preview.2", PRODUCT, false);
    let release = release("1.0.6-preview.2", &bytes);
    let server = Server::new(vec![Reply::body(bytes.clone())]);
    let c = server.client(&temp);
    let values = Arc::new(Mutex::new(Vec::new()));
    let logged = values.clone();
    let download = c
        .download(
            &release,
            Some(Arc::new(move |n| logged.lock().unwrap().push(n))),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let path = download.path().to_path_buf();
    assert!(path.is_absolute());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(server.count(), 1);
    assert!(server.requests.lock().unwrap()[0].starts_with("GET /https://github.com/"));
    assert_eq!(*values.lock().unwrap().first().unwrap(), 0);
    assert_eq!(*values.lock().unwrap().last().unwrap(), 100);
    let parent = path.parent().unwrap().to_path_buf();
    drop(c);
    assert!(path.exists());
    drop(download);
    assert!(!parent.exists());
}
#[tokio::test]
async fn bad_proxy_bytes_fall_back_once_and_failures_leave_no_payload() {
    let temp = Temp::new();
    let bytes = executable("1.0.6-preview.2", PRODUCT, false);
    let release = release("1.0.6-preview.2", &bytes);
    let mut corrupt = bytes.clone();
    corrupt[0] = 0;
    let server = Server::new(vec![
        Reply::body(corrupt.clone()),
        Reply::body(bytes.clone()),
    ]);
    let d = server
        .client(&temp)
        .download(&release, None, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(server.count(), 2);
    drop(d);
    assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 0);
    for body in [
        corrupt,
        executable("1.0.6-preview.2", PRODUCT, true),
        executable("1.0.6-preview.2", "LiteTicker", false),
    ] {
        let expected = super::parse_release(&metadata(&Release {
            version: "1.0.6-preview.2".into(),
            sha256: hex(&Sha256::digest(&body)),
            size: body.len() as u64,
            ..release.clone()
        }))
        .unwrap();
        let server = Server::new(vec![Reply::body(body.clone()), Reply::body(body)]);
        assert!(
            server
                .client(&temp)
                .download(&expected, None, CancellationToken::new())
                .await
                .is_err()
        );
        assert_eq!(server.count(), 2);
        assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 0);
    }
}
#[tokio::test]
async fn cancellation_in_request_or_after_success_progress_never_returns_payload() {
    let temp = Temp::new();
    let bytes = executable("1.0.6-preview.2", PRODUCT, false);
    let release = release("1.0.6-preview.2", &bytes);
    let server = Server::new(vec![Reply {
        delay: Duration::from_millis(150),
        ..Reply::body(bytes.clone())
    }]);
    let token = CancellationToken::new();
    let cancel = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(25)).await;
        cancel.cancel();
    });
    assert!(
        server
            .client(&temp)
            .download(&release, None, token)
            .await
            .unwrap_err()
            .to_string()
            .contains("取消")
    );
    assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 0);
    let server = Server::new(vec![Reply::body(bytes)]);
    let token = CancellationToken::new();
    let cancel = token.clone();
    assert!(
        server
            .client(&temp)
            .download(
                &release,
                Some(Arc::new(move |n| if n == 100 {
                    cancel.cancel();
                })),
                token
            )
            .await
            .is_err()
    );
    assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 0);
}
fn plan(temp: &Temp) -> Plan {
    let old = executable(crate::VERSION, PRODUCT, false);
    let new = executable("1.0.6-preview.2", PRODUCT, false);
    let target = temp.0.join(ASSET);
    let payload = temp.0.join("payload.exe");
    fs::write(&target, &old).unwrap();
    fs::write(&payload, &new).unwrap();
    Plan {
        target,
        payload,
        version: "1.0.6-preview.2".into(),
        sha256: hex(&Sha256::digest(new)),
        size: fs::metadata(temp.0.join("payload.exe")).unwrap().len(),
        original_sha256: hex(&Sha256::digest(old)),
        original_version: crate::VERSION.into(),
        parent_id: 1,
        parent_started: 1,
        wait_ms: 20,
    }
}
#[test]
fn atomic_install_preserves_backup_and_rolls_back_on_start_failure() {
    let temp = Temp::new();
    let p = plan(&temp);
    let old = fs::read(&p.target).unwrap();
    let new = fs::read(&p.payload).unwrap();
    apply_plan(
        &p,
        |_| Ok(()),
        |path| {
            assert_eq!(fs::read(path).unwrap(), new);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(
        fs::read(p.target.with_file_name(format!("{ASSET}.previous"))).unwrap(),
        old
    );
    let p = plan(&temp);
    let mut starts = 0;
    let error = apply_plan(
        &p,
        |_| Ok(()),
        |path| {
            starts += 1;
            if starts == 1 {
                bail!("fixture new launch failed");
            }
            assert_eq!(fs::read(path).unwrap(), old);
            Ok(())
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("已恢复"));
    assert_eq!(starts, 2);
    assert_eq!(fs::read(&p.target).unwrap(), old);
}
#[test]
fn parent_wait_or_payload_or_original_tamper_never_replaces_target() {
    let temp = Temp::new();
    let p = plan(&temp);
    let old = fs::read(&p.target).unwrap();
    assert!(apply_plan(&p, |_| bail!("still running"), |_| panic!("must not start")).is_err());
    assert_eq!(fs::read(&p.target).unwrap(), old);
    fs::write(&p.payload, b"corrupt").unwrap();
    assert!(apply_plan(&p, |_| Ok(()), |_| panic!("must not start")).is_err());
    assert_eq!(fs::read(&p.target).unwrap(), old);
    let p = plan(&temp);
    fs::write(&p.target, b"changed current").unwrap();
    assert!(apply_plan(&p, |_| Ok(()), |_| panic!("must not start")).is_err());
    assert_eq!(fs::read(&p.target).unwrap(), b"changed current");
}

#[tokio::test]
async fn deadlines_and_check_cancellation_are_explicit() {
    let result: Result<()> = bounded(
        &CancellationToken::new(),
        Duration::from_millis(10),
        "fixture timeout",
        async { std::future::pending().await },
    )
    .await;
    assert!(result.unwrap_err().to_string().contains("fixture timeout"));
    let temp = Temp::new();
    let server = Server::new(vec![Reply {
        delay: Duration::from_millis(150),
        ..Reply::body(b"[]".to_vec())
    }]);
    let client = server.client(&temp);
    let token = CancellationToken::new();
    let abort = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(25)).await;
        abort.cancel();
    });
    assert!(
        client
            .check(token)
            .await
            .unwrap_err()
            .to_string()
            .contains("取消")
    );
}
#[tokio::test]
async fn unsafe_redirects_are_rejected_before_following_and_official_fallback_is_bounded() {
    let temp = Temp::new();
    let bytes = executable("1.0.6-preview.2", PRODUCT, false);
    let release = release("1.0.6-preview.2", &bytes);
    let server = Server::new(vec![
        Reply {
            status: 302,
            location: Some("https://evil.example/asset".into()),
            ..Reply::body(vec![])
        },
        Reply::body(bytes.clone()),
    ]);
    let download = server
        .client(&temp)
        .download(&release, None, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(server.count(), 2);
    drop(download);
    assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 0);
    let mut replies = (0..4)
        .map(|_| Reply {
            status: 302,
            location: Some("https://release-assets.githubusercontent.com/asset".into()),
            ..Reply::body(vec![])
        })
        .collect::<Vec<_>>();
    replies.push(Reply::body(bytes));
    let server = Server::new(replies);
    let download = server
        .client(&temp)
        .download(&release, None, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(server.count(), 5);
    drop(download);
}
#[test]
fn identity_checks_file_version_original_filename_and_handoff_arguments() {
    let bytes = executable("1.0.6-preview.2", PRODUCT, false);
    let signature = 0xFEEF04BD_u32.to_le_bytes();
    let index = bytes.windows(4).position(|s| s == signature).unwrap();
    let mut wrong = bytes.clone();
    put32(&mut wrong, index + 12, 7 << 16);
    assert!(native_identity(&wrong).is_err());
    let name = utf16(ASSET);
    let index = bytes.windows(name.len()).position(|s| s == name).unwrap();
    let mut wrong = bytes.clone();
    put16(&mut wrong, index, b'X' as u16);
    assert!(native_identity(&wrong).is_err());
    assert!(run_helper(&[]).is_err());
    assert!(!owned_container(Path::new("/not-owned")));
    let temp = Temp::new();
    let unrelated = temp.0.join("other.txt");
    fs::write(&unrelated, b"keep").unwrap();
    {
        let dir = OwnedDirectory::new_in(&temp.0).unwrap();
        fs::write(dir.path.join(ASSET), b"own").unwrap();
    }
    assert_eq!(fs::read(unrelated).unwrap(), b"keep");
}
#[test]
#[ignore = "requires an explicitly supplied, separately built native preview EXE; never launches it"]
fn actual_native_preview_identity() {
    let path = PathBuf::from(
        std::env::var_os("LITETICKER_NATIVE_PROBE")
            .expect("set LITETICKER_NATIVE_PROBE to an owned native preview artifact"),
    );
    let bytes = read_file_bounded(&path, LIMIT).unwrap();
    let identity = native_identity(&bytes).unwrap();
    assert_eq!(identity.version, version(crate::VERSION).unwrap());
    println!(
        "native preview identity OK; bytes={}; sha256={}",
        bytes.len(),
        hex(&Sha256::digest(&bytes))
    );
}

#[cfg(unix)]
#[test]
fn cleanup_refuses_a_replaced_directory_symlink() {
    let temp = Temp::new();
    let outside = Temp::new();
    fs::write(outside.0.join(ASSET), b"foreign").unwrap();
    let owned = OwnedDirectory::new_in(&temp.0).unwrap();
    let path = owned.path.clone();
    fs::remove_dir(&path).unwrap();
    std::os::unix::fs::symlink(&outside.0, &path).unwrap();
    drop(owned);
    assert_eq!(fs::read(outside.0.join(ASSET)).unwrap(), b"foreign");
    assert!(path.is_symlink());
    fs::remove_file(path).unwrap();
}
