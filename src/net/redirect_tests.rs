use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::{self, JoinHandle};

use super::*;

fn test_agent() -> ureq::Agent {
    loopback_agent(|config| {
        config
            .timeout_global(Some(Duration::from_secs(5)))
            .max_redirects(10)
    })
}

fn request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut reader = BufReader::new(stream);
    let mut request = String::new();
    loop {
        let start = request.len();
        assert_ne!(reader.read_line(&mut request).unwrap(), 0);
        if &request[start..] == "\r\n" {
            return request.to_ascii_lowercase();
        }
    }
}

fn serve(replies: Vec<String>) -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let worker = thread::spawn(move || {
        let started = Instant::now();
        replies
            .into_iter()
            .map(|reply| {
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            assert!(started.elapsed() < Duration::from_secs(5));
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("could not accept loopback request: {error}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                let received = request(&mut stream);
                stream
                    .write_all(format!("HTTP/1.1 {reply}\r\nConnection: close\r\n\r\n").as_bytes())
                    .unwrap();
                received
            })
            .collect()
    });
    (url, worker)
}

#[test]
fn scoped_headers_match_only_the_configured_https_origin() {
    let origin = Url::parse("https://updates.example.test/releases/check").unwrap();
    let authorized = [("X-Test-Scope", "fixture")];
    let headers = ScopedHeaders::new(origin.as_str(), &authorized);
    let relative = origin.join("../current").unwrap();
    assert_eq!(headers.for_url(&relative), &authorized);
    let default_port = origin
        .join("https://updates.example.test:443/current")
        .unwrap();
    assert_eq!(headers.for_url(&default_port), &authorized);
    for location in [
        "https://cdn.example.test/current",
        "https://updates.example.test:8443/current",
        "http://updates.example.test/current",
    ] {
        assert!(headers.for_url(&origin.join(location).unwrap()).is_empty());
    }
    let insecure = ScopedHeaders::new("http://updates.example.test/", &authorized);
    assert!(insecure.for_url(&relative).is_empty());
}

#[test]
fn relative_and_cross_origin_redirects_preserve_get_and_public_headers() {
    let (other_origin, other_server) =
        serve(vec!["206 Partial Content\r\nContent-Length: 0".into()]);
    let (origin, server) = serve(vec![
        "303 See Other\r\nLocation: ../current\r\nContent-Length: 0".into(),
        format!("307 Temporary Redirect\r\nLocation: {other_origin}/asset\r\nContent-Length: 0"),
    ]);
    let https_origin = origin.replacen("http://", "https://", 1);
    let (final_url, response) = get_following_redirects(
        &test_agent(),
        &format!("{origin}/releases/previous"),
        &[("If-None-Match", "\"fixture\""), ("Range", "bytes=4-7")],
        ScopedHeaders::new(&https_origin, &[("X-Test-Scope", "fixture")]),
    )
    .unwrap();
    assert_eq!(final_url.as_str(), format!("{other_origin}/asset"));
    assert_eq!(response.status(), 206);
    let requests = server
        .join()
        .unwrap()
        .into_iter()
        .chain(other_server.join().unwrap());
    for (request, path) in requests.zip(["/releases/previous", "/current", "/asset"]) {
        assert!(request.starts_with(&format!("get {path} http/1.1\r\n")));
        assert!(request.contains("\r\nif-none-match: \"fixture\"\r\n"));
        assert!(request.contains("\r\nrange: bytes=4-7\r\n"));
        assert!(!request.contains("\r\nx-test-scope:"));
    }
}

#[test]
fn a_not_modified_response_ends_the_redirect_chain() {
    let (origin, server) = serve(vec![
        "302 Found\r\nLocation: /current\r\nContent-Length: 0".into(),
        "304 Not Modified".into(),
    ]);
    let (_, response) = get_following_redirects(
        &test_agent(),
        &format!("{origin}/previous"),
        &[("If-None-Match", "\"fixture\"")],
        ScopedHeaders::new(&origin, &[]),
    )
    .unwrap();
    assert_eq!(response.status(), 304);
    let requests = server.join().unwrap();
    assert!(requests[1].contains("\r\nif-none-match: \"fixture\"\r\n"));
}

#[test]
fn redirect_chain_stops_after_ten_hops() {
    let (origin, server) = serve(vec![
        "308 Permanent Redirect\r\nLocation: /again\r\nContent-Length: 0".into();
        11
    ]);
    let result = get_following_redirects(
        &test_agent(),
        &origin,
        &[],
        ScopedHeaders::new(&origin, &[]),
    );
    assert!(matches!(result, Err(ureq::Error::TooManyRedirects)));
    assert_eq!(server.join().unwrap().len(), 11);
}
