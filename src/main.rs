mod chunker;
mod livekit;
mod tts_pool;

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
        ("POST", "/chat") => handle_chat(&mut stream, &body),
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

/// Splits the input text into sentence-level chunks and synthesizes them in
/// parallel using the TTS worker pool.  Audio shards are streamed back in order
/// via HTTP chunked transfer encoding ("speak early, fill later").
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

    let chunks = chunker::chunk_text(&text);
    println!("text split into {} chunk(s)", chunks.len());

    tts_pool::stream_parallel(stream, &chunks, &tts_url)
}

/// Orchestrator endpoint: accepts user text, calls the LLM to produce a
/// response, then synthesizes it through the parallel TTS pipeline.
///
/// Architecture (from the "Voice stack: brain + TTS pool" diagram):
///   User text → LLM orchestrator → response text → chunker → TTS pool → audio stream
fn handle_chat(stream: &mut TcpStream, body: &str) -> std::io::Result<()> {
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

    let Some(llm_url) = env::var("LLM_URL").ok().filter(|url| !url.is_empty()) else {
        return send_response(
            stream,
            Response::json(
                "500 Internal Server Error",
                "{\"error\":\"LLM_URL is not configured\"}",
            ),
        );
    };
    let Some(tts_url) = env::var("TTS_URL").ok().filter(|url| !url.is_empty()) else {
        return send_response(
            stream,
            Response::json(
                "500 Internal Server Error",
                "{\"error\":\"TTS_URL is not configured\"}",
            ),
        );
    };

    println!("chat: calling LLM …");
    let llm_body = match ureq::post(&llm_url)
        .set("Content-Type", "application/json")
        .send_json(ureq::json!({ "text": text }))
    {
        Ok(resp) => match resp.into_string() {
            Ok(s) => s,
            Err(e) => {
                return send_response(
                    stream,
                    Response::json(
                        "502 Bad Gateway",
                        &format!(
                            "{{\"error\":\"LLM response read error: {}\"}}",
                            escape_json(&e.to_string())
                        ),
                    ),
                );
            }
        },
        Err(e) => {
            return send_response(
                stream,
                Response::json(
                    "502 Bad Gateway",
                    &format!(
                        "{{\"error\":\"LLM request failed: {}\"}}",
                        escape_json(&e.to_string())
                    ),
                ),
            );
        }
    };

    let Some(response_text) = json_string_field(&llm_body, "text") else {
        return send_response(
            stream,
            Response::json(
                "502 Bad Gateway",
                "{\"error\":\"LLM response missing 'text' field\"}",
            ),
        );
    };

    println!(
        "chat: LLM responded ({} chars), synthesizing …",
        response_text.len()
    );

    let chunks = chunker::chunk_text(&response_text);
    println!("text split into {} chunk(s)", chunks.len());

    tts_pool::stream_parallel(stream, &chunks, &tts_url)
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
