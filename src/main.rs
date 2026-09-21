mod livekit;

use std::{
    env,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
};

fn main() -> std::io::Result<()> {
    let address = env::var("ADDRESS").unwrap_or_else(|_| "127.0.0.1:7878".to_owned());
    let listener = TcpListener::bind(&address)?;
    println!("TTS server listening on http://{address}");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(error) = handle_connection(stream) {
                    eprintln!("request failed: {error}");
                }
            }
            Err(error) => eprintln!("connection failed: {error}"),
        }
    }

    Ok(())
}

fn handle_connection(mut stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }

    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    let body = String::from_utf8_lossy(&body).into_owned();

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();

    match (method, path) {
        ("GET", "/token") => send_response(&mut stream, token_response()),
        ("POST", "/speak") => stream_tts(&mut stream, &body),
        ("OPTIONS", _) => send_response(&mut stream, Response::empty("204 No Content")),
        _ => send_response(
            &mut stream,
            Response::json("404 Not Found", "{\"error\":\"not found\"}"),
        ),
    }
}

struct Response {
    status: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

impl Response {
    fn json(status: &'static str, body: &str) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: body.as_bytes().to_vec(),
        }
    }

    fn empty(status: &'static str) -> Self {
        Self {
            status,
            content_type: "text/plain",
            body: Vec::new(),
        }
    }
}

fn token_response() -> Response {
    let room = env::var("LIVEKIT_ROOM").unwrap_or_else(|_| "tts-room".to_owned());
    match livekit::create_token("tts-bot", &room) {
        Ok(token) => Response::json(
            "200 OK",
            &format!(
                "{{\"token\":\"{}\",\"room\":\"{}\"}}",
                escape_json(&token),
                escape_json(&room)
            ),
        ),
        Err(error) => Response::json(
            "500 Internal Server Error",
            &format!("{{\"error\":\"{}\"}}", escape_json(&error.to_string())),
        ),
    }
}

/// Calls the configured TTS service and forwards its response to React as HTTP chunks.
/// The TTS service must return audio bytes (for example, audio/mpeg or audio/wav).
fn stream_tts(stream: &mut TcpStream, body: &str) -> std::io::Result<()> {
    let Some(text) = json_string_field(body, "text") else {
        return send_response(
            stream,
            Response::json(
                "400 Bad Request",
                "{\"error\":\"JSON field 'text' is required\"}",
            ),
        );
    };
    if text.trim().is_empty() {
        return send_response(
            stream,
            Response::json("400 Bad Request", "{\"error\":\"text cannot be empty\"}"),
        );
    }

    let Some(tts_url) = env::var("TTS_URL").ok().filter(|url| !url.is_empty()) else {
        return send_response(
            stream,
            Response::json(
                "500 Internal Server Error",
                "{\"error\":\"TTS_URL is not configured\"}",
            ),
        );
    };

    let result = ureq::post(&tts_url)
        .set("Content-Type", "application/json")
        .send_json(ureq::json!({ "text": text }));

    let response = match result {
        Ok(response) => response,
        Err(error) => {
            return send_response(
                stream,
                Response::json(
                    "502 Bad Gateway",
                    &format!("{{\"error\":\"{}\"}}", escape_json(&error.to_string())),
                ),
            );
        }
    };

    let content_type = response.header("Content-Type").unwrap_or_default();
    if !content_type.to_ascii_lowercase().contains("audio/mpeg")
        && !content_type.to_ascii_lowercase().contains("audio/mp3")
    {
        return send_response(
            stream,
            Response::json(
                "502 Bad Gateway",
                &format!(
                    "{{\"error\":\"TTS provider must return MP3 audio (audio/mpeg), received '{}'\"}}",
                    escape_json(content_type)
                ),
            ),
        );
    }

    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nContent-Disposition: inline; filename=tts.mp3\r\nTransfer-Encoding: chunked\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: Content-Type\r\nConnection: close\r\n\r\n"
    )?;

    let mut reader = response.into_reader();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        write!(stream, "{count:X}\r\n")?;
        stream.write_all(&buffer[..count])?;
        stream.write_all(b"\r\n")?;
        stream.flush()?;
    }
    stream.write_all(b"0\r\n\r\n")?;
    Ok(())
}

fn send_response(stream: &mut TcpStream, response: Response) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: Content-Type\r\nConnection: close\r\n\r\n",
        response.status,
        response.content_type,
        response.body.len()
    )?;
    stream.write_all(&response.body)
}

fn json_string_field(body: &str, field: &str) -> Option<String> {
    let marker = format!("\"{field}\"");
    let start = body.find(&marker)?;
    let value = body[start + marker.len()..].split_once(':')?.1.trim_start();
    if !value.starts_with('"') {
        return None;
    }
    let mut escaped = false;
    let mut result = String::new();
    for character in value[1..].chars() {
        if escaped {
            result.push(match character {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '"' {
            return Some(result);
        } else {
            result.push(character);
        }
    }
    None
}

fn escape_json(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}
