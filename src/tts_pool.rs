use std::collections::HashMap;
use std::env;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

const DEFAULT_WORKERS: usize = 4;

/// Synthesizes chunks in parallel using a thread-pool work-stealing pattern,
/// then streams the resulting audio shards in order via HTTP chunked transfer.
///
/// Architecture (from the "Parallel TTS shards" diagram):
///   1. N worker threads each grab the next unprocessed chunk (atomic counter).
///   2. Each worker calls the TTS provider and sends (index, audio_bytes) to a channel.
///   3. The main thread keeps an ordered buffer: whenever the *next expected* shard
///      arrives it flushes all consecutive ready shards to the HTTP stream.
///   4. Wall-clock time ≈ max(t_1 … t_N) instead of sum.
pub fn stream_parallel(
    stream: &mut TcpStream,
    chunks: &[String],
    tts_url: &str,
) -> std::io::Result<()> {
    let chunk_count = chunks.len();
    if chunk_count == 0 {
        return Ok(());
    }

    let max_workers: usize = env::var("TTS_WORKERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_WORKERS);
    let worker_count = max_workers.min(chunk_count);

    println!(
        "parallel TTS: {} chunk(s), {} worker(s)",
        chunk_count, worker_count
    );

    write!(
        stream,
        "HTTP/1.1 200 OK\r\n\
         Content-Type: audio/mpeg\r\n\
         Content-Disposition: inline; filename=tts.mp3\r\n\
         Transfer-Encoding: chunked\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Headers: Content-Type\r\n\
         Connection: close\r\n\r\n"
    )?;

    let chunks: Arc<[String]> = chunks.to_vec().into();
    let tts_url: Arc<str> = tts_url.into();
    let next_work = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = mpsc::channel::<(usize, Result<Vec<u8>, String>)>();

    for _ in 0..worker_count {
        let sender = tx.clone();
        let chunks = Arc::clone(&chunks);
        let url = Arc::clone(&tts_url);
        let nw = Arc::clone(&next_work);
        thread::spawn(move || {
            loop {
                let idx = nw.fetch_add(1, Ordering::Relaxed);
                if idx >= chunks.len() {
                    break;
                }
                let result = call_tts(&url, &chunks[idx]);
                if sender.send((idx, result)).is_err() {
                    break;
                }
            }
        });
    }
    drop(tx);

    let mut buffer: HashMap<usize, Vec<u8>> = HashMap::new();
    let mut next_index: usize = 0;

    for (idx, result) in rx {
        match result {
            Ok(data) => {
                buffer.insert(idx, data);
                while let Some(shard) = buffer.remove(&next_index) {
                    write_http_chunk(stream, &shard)?;
                    println!("  streamed shard {}/{}", next_index + 1, chunk_count);
                    next_index += 1;
                }
            }
            Err(e) => {
                eprintln!("TTS error for chunk {idx}: {e}");
                break;
            }
        }
    }

    stream.write_all(b"0\r\n\r\n")?;
    stream.flush()
}

fn call_tts(url: &str, text: &str) -> Result<Vec<u8>, String> {
    let response = ureq::post(url)
        .set("Content-Type", "application/json")
        .send_json(ureq::json!({ "text": text }))
        .map_err(|e| format!("TTS request failed: {e}"))?;

    let content_type = response
        .header("Content-Type")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !content_type.contains("audio/mpeg") && !content_type.contains("audio/mp3") {
        return Err(format!(
            "TTS provider returned '{content_type}', expected audio/mpeg"
        ));
    }

    let mut data = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut data)
        .map_err(|e| format!("read TTS response: {e}"))?;
    Ok(data)
}

fn write_http_chunk(stream: &mut TcpStream, data: &[u8]) -> std::io::Result<()> {
    write!(stream, "{:X}\r\n", data.len())?;
    stream.write_all(data)?;
    stream.write_all(b"\r\n")?;
    stream.flush()
}
