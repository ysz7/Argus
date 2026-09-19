//! A minimal HTTP/1.1 server side: one `GET` request per connection.
//!
//! The service answers a handful of read-only requests from local programs,
//! so it needs neither keep-alive nor request bodies. Everything that can be
//! refused early is: oversized or malformed requests, other methods, and
//! requests that do not come from a local program (see [`Request::check`]).

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;

/// Longest accepted request head (request line and headers), in bytes.
const MAX_HEAD: usize = 16 * 1024;

/// A parsed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Request {
    pub(crate) method: String,
    /// The path, percent-decoded, without the query.
    pub(crate) path: String,
    /// Query parameters, percent-decoded, in order.
    pub(crate) query: Vec<(String, String)>,
    /// Headers with lowercase names, in order.
    pub(crate) headers: Vec<(String, String)>,
}

impl Request {
    /// The first value of header `name` (lowercase).
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
    }

    /// The value of query parameter `name`; an error if it is repeated.
    pub(crate) fn param(&self, name: &str) -> Result<Option<&str>, Response> {
        let mut values = self.query.iter().filter(|(key, _)| key == name);
        let first = values.next().map(|(_, value)| value.as_str());
        if values.next().is_some() {
            return Err(Response::error(
                400,
                "bad_request",
                format!("query parameter `{name}` is repeated"),
            ));
        }
        Ok(first)
    }

    /// Refuses requests that may come from a web page rather than a local
    /// program.
    ///
    /// A page in a browser can send requests to localhost. Browsers mark
    /// them with an `Origin` header; and a page whose domain resolves to
    /// 127.0.0.1 (DNS rebinding) still sends its own domain as `Host`. Screen
    /// content must never reach a web page, so both are refused.
    pub(crate) fn check(&self, port: u16) -> Result<(), Response> {
        if self.header("origin").is_some() {
            return Err(Response::error(
                403,
                "forbidden",
                "requests from web pages are not allowed".to_owned(),
            ));
        }
        let host = self.header("host").unwrap_or_default().to_ascii_lowercase();
        let local = ["127.0.0.1", "localhost", "[::1]"].iter().any(|name| {
            host.strip_prefix(name).and_then(|rest| rest.strip_prefix(':'))
                == Some(&port.to_string())
        });
        if !local {
            return Err(Response::error(
                403,
                "forbidden",
                format!("the Host header must be 127.0.0.1:{port} or localhost:{port}"),
            ));
        }
        Ok(())
    }
}

/// Reads one request head from `stream`.
///
/// `Ok(Err(response))` is the answer to a request that cannot be served; an
/// I/O error means the client closed the connection or went silent.
pub(crate) fn read_request(stream: &TcpStream) -> io::Result<Result<Request, Response>> {
    let mut reader = BufReader::new(stream.take(MAX_HEAD as u64 + 1));
    let mut lines = Vec::new();
    let mut size = 0;
    loop {
        let mut line = Vec::new();
        let read = reader.read_until(b'\n', &mut line)?;
        size += read;
        if size > MAX_HEAD {
            return Ok(Err(Response::error(
                431,
                "request_too_large",
                "the request head is too large".to_owned(),
            )));
        }
        if read == 0 || !line.ends_with(b"\n") {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "incomplete request"));
        }
        while line.last().is_some_and(|byte| matches!(byte, b'\n' | b'\r')) {
            line.pop();
        }
        if line.is_empty() {
            if lines.is_empty() {
                continue; // tolerate empty lines before the request line
            }
            break;
        }
        lines.push(line);
    }
    Ok(parse(&lines))
}

fn parse(lines: &[Vec<u8>]) -> Result<Request, Response> {
    let bad = |message: &str| Response::error(400, "bad_request", message.to_owned());
    fn text(line: &[u8]) -> Result<&str, Response> {
        std::str::from_utf8(line)
            .map_err(|_| Response::error(400, "bad_request", "the request is not UTF-8".to_owned()))
    }
    let mut parts = text(&lines[0])?.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(bad("malformed request line"));
    };
    if !version.starts_with("HTTP/1.") {
        return Err(Response::error(
            505,
            "http_version_not_supported",
            "only HTTP/1.x is supported".to_owned(),
        ));
    }
    let mut headers = Vec::new();
    for line in &lines[1..] {
        let (name, value) = text(line)?.split_once(':').ok_or_else(|| bad("malformed header"))?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if !path.starts_with('/') {
        return Err(bad("the request target must be a path"));
    }
    let path = decode(path, false).ok_or_else(|| bad("malformed percent-encoding"))?;
    let query = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            Some((decode(key, true)?, decode(value, true)?))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| bad("malformed percent-encoding"))?;
    Ok(Request { method: method.to_owned(), path, query, headers })
}

/// Percent-decodes `text`; in a query, `+` stands for a space.
fn decode(text: &str, query: bool) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                index += 3;
            }
            b'+' if query => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// A response with a JSON body.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Response {
    pub(crate) status: u16,
    pub(crate) body: serde_json::Value,
    /// Extra headers.
    pub(crate) headers: Vec<(&'static str, String)>,
    /// A non-JSON body and its content type, sent instead of `body`.
    pub(crate) raw: Option<(&'static str, Vec<u8>)>,
}

impl Response {
    pub(crate) fn ok(body: serde_json::Value) -> Self {
        Self { status: 200, body, headers: Vec::new(), raw: None }
    }

    /// A successful response with a binary body (e.g. `image/png`).
    pub(crate) fn bytes(content_type: &'static str, data: Vec<u8>) -> Self {
        Self { raw: Some((content_type, data)), ..Self::ok(serde_json::Value::Null) }
    }

    /// An error response: `{"error": {"code": ..., "message": ...}}`.
    pub(crate) fn error(status: u16, code: &str, message: String) -> Self {
        Self::ok(serde_json::json!({ "error": { "code": code, "message": message } }))
            .with_status(status)
    }

    pub(crate) fn with_status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    pub(crate) fn with_header(mut self, name: &'static str, value: String) -> Self {
        self.headers.push((name, value));
        self
    }

    /// The error code of an error response.
    pub(crate) fn code(&self) -> Option<&str> {
        self.body.get("error")?.get("code")?.as_str()
    }

    pub(crate) fn write(&self, mut stream: impl Write) -> io::Result<()> {
        let (content_type, body) = match &self.raw {
            Some((content_type, data)) => (*content_type, data.clone()),
            None => ("application/json", serde_json::to_vec(&self.body).map_err(io::Error::other)?),
        };
        let mut head = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
             Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n",
            self.status,
            reason(self.status),
            body.len()
        );
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes())?;
        stream.write_all(&body)?;
        stream.flush()
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(head: &str) -> Result<Request, Response> {
        let lines: Vec<Vec<u8>> = head.lines().map(|line| line.as_bytes().to_vec()).collect();
        parse(&lines)
    }

    #[test]
    fn parses_paths_and_queries() {
        let request =
            request("GET /v1/observation?app=Proxy+Generator%20Lite&sources=ocr HTTP/1.1\nHost: x")
                .unwrap();
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/v1/observation");
        assert_eq!(request.param("app").unwrap(), Some("Proxy Generator Lite"));
        assert_eq!(request.param("sources").unwrap(), Some("ocr"));
        assert_eq!(request.param("pid").unwrap(), None);
        assert_eq!(request.header("host"), Some("x"));
    }

    #[test]
    fn rejects_malformed_requests() {
        assert_eq!(request("GET /").unwrap_err().status, 400);
        assert_eq!(request("GET / HTTP/2").unwrap_err().status, 505);
        assert_eq!(request("GET /%zz HTTP/1.1").unwrap_err().status, 400);
        assert_eq!(request("GET http://x/ HTTP/1.1").unwrap_err().status, 400);
        let repeated = request("GET /?a=1&a=2 HTTP/1.1").unwrap();
        assert_eq!(repeated.param("a").unwrap_err().status, 400);
    }

    #[test]
    fn only_local_programs_are_served() {
        let check = |head: &str| request(head).unwrap().check(7412).map_err(|r| r.status);
        assert_eq!(check("GET / HTTP/1.1\nHost: 127.0.0.1:7412"), Ok(()));
        assert_eq!(check("GET / HTTP/1.1\nHost: LOCALHOST:7412"), Ok(()));
        assert_eq!(check("GET / HTTP/1.1\nHost: [::1]:7412"), Ok(()));
        // DNS rebinding: a web page's own domain resolved to 127.0.0.1.
        assert_eq!(check("GET / HTTP/1.1\nHost: evil.example:7412"), Err(403));
        assert_eq!(check("GET / HTTP/1.1\nHost: localhost.evil.example:7412"), Err(403));
        assert_eq!(check("GET / HTTP/1.1\nHost: 127.0.0.1:80"), Err(403));
        assert_eq!(check("GET / HTTP/1.1"), Err(403));
        // Any request a browser marks as coming from a page.
        assert_eq!(
            check("GET / HTTP/1.1\nHost: 127.0.0.1:7412\nOrigin: http://127.0.0.1:7412"),
            Err(403)
        );
    }

    #[test]
    fn responses_are_complete() {
        let mut out = Vec::new();
        Response::error(404, "not_found", "no such route".to_owned())
            .with_header("Allow", "GET".to_owned())
            .write(&mut out)
            .unwrap();
        let text = String::from_utf8(out).unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        assert!(head.starts_with("HTTP/1.1 404 Not Found\r\n"), "{head}");
        assert!(head.contains(&format!("Content-Length: {}", body.len())), "{head}");
        assert!(head.contains("Allow: GET"), "{head}");
        assert_eq!(body, r#"{"error":{"code":"not_found","message":"no such route"}}"#);
    }
}
