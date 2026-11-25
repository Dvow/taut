//! Serves a taut program built for the web, with a page and the loader beside it:
//!   cargo build --release --target wasm32-unknown-unknown --example hello
//!   cargo run --example web_page -- target/wasm32-unknown-unknown/release/examples/hello.wasm
//! then open http://127.0.0.1:8000. It writes `index.html` and `taut.js` into the program's
//! folder; any static server can serve that folder instead (a page can't load a program from
//! a `file:` address). A port after the program's path serves on that port.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(program) = args.next().map(PathBuf::from) else {
        return eprintln!("usage: web_page <program.wasm> [port]");
    };
    let port: u16 = args.next().and_then(|port| port.parse().ok()).unwrap_or(8000);
    let folder = program.parent().map(Path::to_path_buf).unwrap_or_default();
    let name = program.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let title = program.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    let written = std::fs::write(folder.join("index.html"), taut::web::page(&title, &name)).and_then(|()| std::fs::write(folder.join("taut.js"), taut::web::LOADER));
    if let Err(error) = written {
        return eprintln!("web_page: {}: {error}", folder.display());
    }
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(error) => return eprintln!("web_page: port {port}: {error}"),
    };
    println!("http://127.0.0.1:{port}/ serves {}", folder.display());
    for stream in listener.incoming().flatten() {
        answer(stream, &folder, &mut |_, _| {});
    }
}

/// What a request asks: its method, path, body, and the bytes it wants (`Range: bytes=`, as
/// a browser asks for a video's), from and up to.
pub struct Asked {
    pub method: String,
    pub path: String,
    pub body: Vec<u8>,
    pub range: Option<(usize, Option<usize>)>,
}

/// Reads one request.
pub fn asked(stream: &mut TcpStream) -> Option<Asked> {
    let mut reader = BufReader::new(stream);
    let mut first = String::new();
    reader.read_line(&mut first).ok()?;
    let mut words = first.split_whitespace();
    let (method, path) = (words.next()?.to_string(), words.next()?.to_string());
    let (mut length, mut range) = (0, None);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 || line.trim().is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').unwrap_or_default();
        if name.eq_ignore_ascii_case("content-length") {
            length = value.trim().parse().unwrap_or(0);
        }
        if name.eq_ignore_ascii_case("range") {
            range = value.trim().strip_prefix("bytes=").and_then(|range| range.split_once('-')).and_then(|(from, to)| Some((from.parse().ok()?, to.parse().ok())));
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;
    Some(Asked { method, path, body, range })
}

/// Answers one request: a file of `folder` (the page for `/`), or for a POST, `posted` hears
/// its path and body.
pub fn answer(mut stream: TcpStream, folder: &Path, posted: &mut dyn FnMut(&str, Vec<u8>)) {
    let Some(asked) = asked(&mut stream) else { return };
    if asked.method == "POST" {
        posted(&asked.path, asked.body);
        let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
        return;
    }
    let path = asked.path.split('?').next().unwrap_or_default().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    // Only files right in the folder: nothing above it.
    let safe = !path.contains("..") && !path.contains('\\') && !path.contains(':');
    let Some(bytes) = std::fs::read(folder.join(path)).ok().filter(|_| safe) else {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    };
    let kind = match path.rsplit('.').next().unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript",
        "wasm" => "application/wasm",
        "ttf" | "otf" => "font/ttf",
        "png" => "image/png",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    };
    let last = bytes.len().saturating_sub(1);
    let (first, last) = asked.range.map_or((0, last), |(from, to)| (from, to.map_or(last, |to| to.min(last))));
    let part = bytes.get(first..=last).unwrap_or_default();
    let status = match asked.range {
        Some(_) => format!("206 Partial Content\r\nContent-Range: bytes {first}-{last}/{}", bytes.len()),
        None => "200 OK".to_string(),
    };
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n", part.len());
    let _ = stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(part));
}
