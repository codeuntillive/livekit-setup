# LiveKit Parallel TTS Streaming Server

A Rust HTTP server that splits text into sentence-level chunks, synthesizes them through a TTS provider **in parallel**, and streams the ordered audio back to the client in real time.

## Architecture

```text
                         ┌──────────────────────────┐
                         │   POST /speak or /chat    │
                         └────────────┬─────────────┘
                                      │
                                      ▼
                              ┌───────────────┐
                              │   Chunker      │
                              │ split on . ! ? │
                              └───┬───┬───┬───┘
                                  │   │   │
                      ┌───────────┘   │   └───────────┐
                      ▼               ▼               ▼
               ┌────────────┐  ┌────────────┐  ┌────────────┐
               │  Worker 1  │  │  Worker 2  │  │  Worker 3  │
               │  TTS call  │  │  TTS call  │  │  TTS call  │
               └─────┬──────┘  └─────┬──────┘  └─────┬──────┘
                     │               │               │
                     └───────┬───────┴───────┬───────┘
                             │               │
                             ▼               ▼
                     ┌─────────────────────────────┐
                     │       Ordered Buffer         │
                     │  collect shards, flush in    │
                     │  order as they become ready  │
                     └──────────────┬──────────────┘
                                    │
                                    ▼
                         HTTP chunked stream
                         (audio/mpeg to client)
```

**Sequential** total time = `t₁ + t₂ + t₃`
**Parallel** total time ≈ `max(t₁, t₂, t₃)`

Chunks are produced in parallel but played in order. The ordered buffer streams each shard the moment the next-in-sequence shard is ready ("speak early, fill later").

## Voice stack (brain + TTS pool)

The `/chat` endpoint implements the full orchestrator flow from the voice stack architecture:

```text
User text ──▶ LLM orchestrator (LLM_URL) ──▶ response text
                                                   │
                                                   ▼
                                             Parallel chunker
                                                   │
                                         ┌─────────┼─────────┐
                                         ▼         ▼         ▼
                                       TTS W1    TTS W2    TTS W3
                                         │         │         │
                                         └────┬────┴────┬────┘
                                              ▼         ▼
                                        Ordered buffer
                                              │
                                              ▼
                                   Stream to client / LiveKit
```

## Requirements

- Rust and Cargo
- A TTS provider that accepts JSON text and returns MP3 bytes
- A LiveKit project if the `/token` endpoint is needed
- An LLM endpoint if the `/chat` endpoint is needed

The Rust project uses:

- `livekit-api` for LiveKit access tokens
- `ureq` for calling the TTS and LLM providers
- `std::thread` worker pool for parallel TTS synthesis

## Installation

Clone the project and enter the directory:

```bash
git clone <your-repository-url>
cd webs
```

Build and verify the project:

```bash
cargo fmt -- --check
cargo check
```

## Configuration

Set the environment variables before starting the server:

```bash
export ADDRESS=127.0.0.1:7878
export TTS_URL=https://your-tts-provider.example/synthesize
export TTS_WORKERS=4
export LLM_URL=https://your-llm-api.example/chat
export LIVEKIT_API_KEY=your_livekit_api_key
export LIVEKIT_API_SECRET=your_livekit_api_secret
export LIVEKIT_ROOM=tts-room
```

Start the server:

```bash
cargo run
```

The server will listen on:

```text
http://127.0.0.1:7878
```

### Configuration variables

| Variable | Required | Description |
|---|---:|---|
| `ADDRESS` | No | Address and port to bind. Defaults to `127.0.0.1:7878`. |
| `TTS_URL` | Yes for `/speak` and `/chat` | URL of the TTS provider endpoint. |
| `TTS_WORKERS` | No | Maximum concurrent TTS worker threads. Defaults to `4`. |
| `LLM_URL` | Yes for `/chat` | URL of the LLM orchestrator endpoint. |
| `LIVEKIT_API_KEY` | Yes for `/token` | LiveKit API key. |
| `LIVEKIT_API_SECRET` | Yes for `/token` | LiveKit API secret. |
| `LIVEKIT_ROOM` | No | LiveKit room name. Defaults to `tts-room`. |

Never commit `LIVEKIT_API_SECRET` to Git.

## How the parallel TTS pool works

1. **Chunker** (`src/chunker.rs`): splits the input text on sentence boundaries (`.` `!` `?` followed by whitespace or end of text).

2. **Worker pool** (`src/tts_pool.rs`): spawns up to `TTS_WORKERS` threads. Each worker grabs the next unprocessed chunk using an atomic counter (work-stealing pattern) and calls the TTS provider.

3. **Ordered buffer**: workers send `(index, audio_bytes)` through a channel. The main thread keeps a `HashMap` buffer and flushes all consecutive ready shards to the HTTP stream as soon as the next expected index arrives.

4. **Streaming**: audio is written to the client using HTTP chunked transfer encoding. Each shard is flushed immediately, so the client can begin playback before all chunks are synthesized.

## API endpoints

### `POST /speak`

Splits text into chunks, synthesizes them in parallel, and streams the combined MP3 audio.

Request:

```bash
curl -N \
  -H "Content-Type: application/json" \
  -d '{"text":"Hello from React. This is a test. How are you?"}' \
  http://127.0.0.1:7878/speak \
  -o output.mp3
```

Successful response:

```http
HTTP/1.1 200 OK
Content-Type: audio/mpeg
Content-Disposition: inline; filename=tts.mp3
Transfer-Encoding: chunked
```

The response body is MP3 data streamed in order as shards complete.

Possible errors:

- `400 Bad Request`: missing or empty `text`
- `500 Internal Server Error`: `TTS_URL` is not configured

### `POST /chat`

Orchestrator endpoint. Sends user text to the LLM, then synthesizes the LLM response through the parallel TTS pipeline.

Request:

```bash
curl -N \
  -H "Content-Type: application/json" \
  -d '{"text":"Tell me about Rust programming"}' \
  http://127.0.0.1:7878/chat \
  -o response.mp3
```

The LLM must return `{"text": "..."}`. The response text is chunked and synthesized through the parallel TTS pool.

Possible errors:

- `400 Bad Request`: missing or empty `text`
- `500 Internal Server Error`: `LLM_URL` or `TTS_URL` is not configured
- `502 Bad Gateway`: the LLM request failed or returned an unexpected format

### `GET /token`

Creates a LiveKit token for the identity `tts-bot` and the configured room.

```bash
curl http://127.0.0.1:7878/token
```

Example response:

```json
{
  "token": "eyJ...",
  "room": "tts-room"
}
```

### `OPTIONS`

CORS preflight requests are supported for browser clients.

## TTS provider contract

The server sends this request to `TTS_URL` for each chunk:

```http
POST /synthesize
Content-Type: application/json
```

```json
{
  "text": "One sentence chunk."
}
```

The provider must return MP3 audio bytes:

```http
HTTP/1.1 200 OK
Content-Type: audio/mpeg
```

`audio/mp3` is also accepted.

## LLM provider contract

The server sends this request to `LLM_URL`:

```json
{
  "text": "User message"
}
```

The provider must return:

```json
{
  "text": "LLM response text that will be chunked and synthesized."
}
```

## React usage

### Stream MP3 bytes from `/speak`

```js
async function speak(text) {
  const response = await fetch("http://127.0.0.1:7878/speak", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ text }),
  });

  if (!response.ok) {
    const error = await response.json().catch(() => ({}));
    throw new Error(error.error || `Request failed: ${response.status}`);
  }

  const mp3Blob = await response.blob();
  const audioUrl = URL.createObjectURL(mp3Blob);
  const audio = new Audio(audioUrl);
  await audio.play();
  audio.addEventListener("ended", () => URL.revokeObjectURL(audioUrl));
}
```

### Read chunks as they arrive

```js
async function readAudioChunks(text, onChunk) {
  const response = await fetch("http://127.0.0.1:7878/speak", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ text }),
  });

  if (!response.ok) throw new Error(await response.text());

  const reader = response.body.getReader();
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    onChunk(value); // Uint8Array of MP3 bytes
  }
}
```

## LiveKit usage

Fetch a token from the backend:

```js
const response = await fetch("http://127.0.0.1:7878/token");
const { token, room } = await response.json();
```

Then connect from React using the LiveKit client SDK:

```js
import { Room } from "livekit-client";

const livekitRoom = new Room();
await livekitRoom.connect(import.meta.env.VITE_LIVEKIT_URL, token);
```

This backend streams MP3 over HTTP. It does not publish the generated MP3 into LiveKit by itself. To send TTS audio through LiveKit, a LiveKit publishing participant must connect to the room and publish audio frames.

## Measuring latency

```bash
curl -N \
  -w "\nHTTP: %{http_code}\nTTFB: %{time_starttransfer}s\nTotal: %{time_total}s\nBytes: %{size_download}\n" \
  -H "Content-Type: application/json" \
  -d '{"text":"First sentence. Second sentence. Third sentence."}' \
  http://127.0.0.1:7878/speak \
  -o output.mp3
```

With 3 chunks and a TTS provider that takes 3 seconds per chunk:

| Mode | Total time |
|---|---|
| Sequential (old) | ~9 s (`3 + 3 + 3`) |
| Parallel (new, 3 workers) | ~3 s (`max(3, 3, 3)`) |

## Development checks

```bash
cargo fmt -- --check
cargo check
cargo test
cargo run
```

## Security notes

- Do not expose `LIVEKIT_API_SECRET` to React or browser code.
- Add authentication before exposing `/speak` or `/chat` publicly.
- Add request size limits before deploying to production.
- Restrict CORS from `*` to your real frontend origin in production.
- Add rate limiting to prevent TTS/LLM provider abuse and unexpected costs.
- Use HTTPS in production.
