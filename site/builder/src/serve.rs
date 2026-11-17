//! A small local preview server for website/. It mirrors the production
//! routing that matters for review: directory indexes, the install.html
//! redirect, and the 404 page. It listens on loopback only.
use crate::Result;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    path::{Component, Path, PathBuf},
};

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "mp4" => "video/mp4",
        "woff2" => "font/woff2",
        "txt" | "sha256" => "text/plain; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn respond(stream: &mut TcpStream, status: &str, kind: &str, extra: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n{extra}Connection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}

fn resolve(root: &Path, url: &str) -> Option<PathBuf> {
    let path = url.split(['?', '#']).next().unwrap_or("/");
    let mut file = root.to_path_buf();
    for component in Path::new(path.trim_start_matches('/')).components() {
        match component {
            Component::Normal(part) => file.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if path.ends_with('/') {
        file.push("index.html");
    }
    Some(file)
}

fn handle(root: &Path, mut stream: TcpStream) {
    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).is_err() {
        return;
    }
    let mut parts = line.split_whitespace();
    let (Some(method), Some(url)) = (parts.next(), parts.next()) else {
        return;
    };
    if method != "GET" && method != "HEAD" {
        respond(&mut stream, "405 Method Not Allowed", "text/plain", "", b"");
        return;
    }
    if url == "/install.html" {
        respond(
            &mut stream,
            "301 Moved Permanently",
            "text/plain",
            "Location: /download/\r\n",
            b"",
        );
        return;
    }
    let Some(file) = resolve(root, url) else {
        respond(
            &mut stream,
            "400 Bad Request",
            "text/plain",
            "",
            b"Bad request",
        );
        return;
    };
    if file.is_dir() {
        let location = format!("Location: {}/\r\n", url.trim_end_matches('/'));
        respond(
            &mut stream,
            "301 Moved Permanently",
            "text/plain",
            &location,
            b"",
        );
        return;
    }
    match fs::read(&file) {
        Ok(body) => {
            let body = if method == "HEAD" { Vec::new() } else { body };
            respond(&mut stream, "200 OK", content_type(&file), "", &body);
        }
        Err(_) => {
            let body = fs::read(root.join("404.html")).unwrap_or_else(|_| b"Not found".to_vec());
            respond(
                &mut stream,
                "404 Not Found",
                "text/html; charset=utf-8",
                "",
                &body,
            );
        }
    }
}

pub fn run(root: &Path, port: u16) -> Result<()> {
    if !root.join("index.html").is_file() {
        return Err("website/ has no index.html. Run the `build` or `pages` command first.".into());
    }
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|error| format!("Cannot listen on 127.0.0.1:{port}: {error}"))?;
    println!(
        "Serving {} at http://127.0.0.1:{port}/ (Ctrl+C stops)",
        root.display()
    );
    for stream in listener.incoming().flatten() {
        let root = root.to_path_buf();
        std::thread::spawn(move || handle(&root, stream));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_stay_inside_the_site() {
        let root = Path::new("/site");
        assert_eq!(
            resolve(root, "/guide/"),
            Some(PathBuf::from("/site/guide/index.html"))
        );
        assert_eq!(
            resolve(root, "/a.css?v=1"),
            Some(PathBuf::from("/site/a.css"))
        );
        assert_eq!(resolve(root, "/../etc/passwd"), None);
        assert_eq!(resolve(root, "/assets/../../x"), None);
    }
}
