# LiveKit Text-to-Speech Streaming Server

A small Rust HTTP server for a React text-to-speech frontend.

The application accepts text from the frontend, sends it to a configured TTS provider, and streams the returned MP3 audio back to the browser chunk by chunk. It also provides a LiveKit access-token endpoint so a React client can join the configured LiveKit room.

## What it does

```text
React frontend
     |
     | POST /speak { "text": "..." }
     v
Rust server
     |
     | POST configured TTS_URL
     v
TTS provider
     |
     | audio/mpeg response
     v
Rust server streams MP3 chunks
     |
     v
React frontend
```

The server does not store the generated audio. Audio is forwarded as it is received from the TTS provider.

## Requirements

- Rust and Cargo
- A TTS provider that accepts JSON text and returns MP3 bytes
- A LiveKit project if the `/token` endpoint is needed

The Rust project uses:

- `livekit-api` for LiveKit access tokens
- `ureq` for calling the TTS provider
- A small HTTP server implemented with Rust's standard library

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
export LIVEKIT_ROOM=tts-room
export LIVEKIT_API_KEY=your_livekit_api_key
export LIVEKIT_API_SECRET=your_livekit_api_secret
export TTS_URL=https://your-tts-provider.example/synthesize
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
| `TTS_URL` | Yes for `/speak` | URL of the TTS provider endpoint. |
| `LIVEKIT_API_KEY` | Yes for `/token` | LiveKit API key. |
| `LIVEKIT_API_SECRET` | Yes for `/token` | LiveKit API secret. |
| `LIVEKIT_ROOM` | No | LiveKit room name. Defaults to `tts-room`. |

Never commit `LIVEKIT_API_SECRET` to Git.

## TTS provider contract

The Rust server sends this request to `TTS_URL`:

```http
POST /synthesize
Content-Type: application/json
```

```json
{
  "text": "Hello from React"
}
```

The provider must return MP3 audio bytes:

```http
HTTP/1.1 200 OK
Content-Type: audio/mpeg
```

`audio/mp3` is also accepted.

The provider can return the MP3 as a normal response body. The Rust server reads it and forwards it to the frontend using HTTP chunked transfer encoding.

## API endpoints

### `POST /speak`

Converts text to MP3 audio and streams the result to the client.

Request:

```bash
curl -N \
  -H "Content-Type: application/json" \
  -d '{"text":"Hello from React"}' \
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

The response body is MP3 data, not JSON.

Verify the downloaded file:

```bash
file output.mp3
```

Possible errors:

- `400 Bad Request`: missing or empty `text`
- `502 Bad Gateway`: the TTS provider failed or returned a non-MP3 response
- `500 Internal Server Error`: `TTS_URL` is not configured

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

The token allows the client to join the room and publish or subscribe to audio tracks. The server currently generates the token; the React application is responsible for using it to connect to LiveKit.

### `OPTIONS`

CORS preflight requests are supported for browser clients.

## React usage

### Stream MP3 bytes from `/speak`

```js
async function speak(text) {
  const response = await fetch("http://127.0.0.1:7878/speak", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ text }),
  });

  if (!response.ok) {
    const error = await response.json().catch(() => ({}));
    throw new Error(error.error || `Request failed: ${response.status}`);
  }

  // This downloads the complete MP3 after the stream finishes.
  const mp3Blob = await response.blob();
  const audioUrl = URL.createObjectURL(mp3Blob);
  const audio = new Audio(audioUrl);
  await audio.play();

  audio.addEventListener("ended", () => {
    URL.revokeObjectURL(audioUrl);
  });
}
```

Use it from a React component:

```jsx
function SpeakButton() {
  const [loading, setLoading] = React.useState(false);

  async function handleSpeak() {
    setLoading(true);
    try {
      await speak("Hello from the React frontend");
    } catch (error) {
      console.error(error);
    } finally {
      setLoading(false);
    }
  }

  return (
    <button onClick={handleSpeak} disabled={loading}>
      {loading ? "Generating..." : "Speak"}
    </button>
  );
}
```

### Read chunks as they arrive

The browser receives the response as a stream:

```js
async function readAudioChunks(text, onChunk) {
  const response = await fetch("http://127.0.0.1:7878/speak", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ text }),
  });

  if (!response.ok) {
    throw new Error(await response.text());
  }

  const reader = response.body.getReader();

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;

    // value is a Uint8Array containing the next MP3 bytes.
    onChunk(value);
  }
}
```

For reliable playback while chunks are arriving, use the browser `MediaSource` API with a codec supported by the returned MP3 stream. For simpler playback, collect the chunks into a `Blob` and create an object URL as shown above.

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

The LiveKit URL is separate from the backend URL. For example:

```env
VITE_LIVEKIT_URL=wss://your-project.livekit.cloud
```

This backend currently streams MP3 over HTTP. It does not publish the generated MP3 into LiveKit by itself. To send TTS audio through LiveKit, a LiveKit publishing participant must connect to the room and publish audio frames. The `/token` endpoint provides the permissions needed for that participant or a frontend publisher.

## Measuring latency

Measure time to first byte and total request time with `curl`:

```bash
curl -N \
  -w "\nHTTP: %{http_code}\nTTFB: %{time_starttransfer}s\nTotal: %{time_total}s\nBytes: %{size_download}\n" \
  -H "Content-Type: application/json" \
  -d '{"text":"Hello from React"}' \
  http://127.0.0.1:7878/speak \
  -o output.mp3
```

Important measurements:

- **TTFB**: time until the first response bytes arrive. This includes the Rust server request and the TTS provider's initial response delay.
- **Total**: time until the complete MP3 is received.
- **Bytes**: size of the returned MP3 file.

A `502` response means the Rust server was reachable, but the TTS provider failed or returned a response that was not MP3 audio.

## Troubleshooting

### `curl: Failed to connect to 127.0.0.1:7878`

The server is not running or is listening on another address.

Start it with:

```bash
cargo run
```

Check that it prints:

```text
TTS server listening on http://127.0.0.1:7878
```

### `TTS_URL is not configured`

Set the variable before starting the server:

```bash
export TTS_URL=https://your-tts-provider.example/synthesize
cargo run
```

### `TTS provider must return MP3 audio`

Inspect the provider response headers. It must return `audio/mpeg` or `audio/mp3`.

### `/speak` returns `502 Bad Gateway`

Test the TTS provider directly:

```bash
curl -v \
  -H "Content-Type: application/json" \
  -d '{"text":"test"}' \
  "$TTS_URL" \
  -o provider-output.mp3
```

Confirm that:

- The URL is correct.
- The provider is reachable.
- Any required provider API key is configured in the provider request.
- The response status is successful.
- The response content type is `audio/mpeg` or `audio/mp3`.

## Development checks

Run formatting and compilation checks:

```bash
cargo fmt -- --check
cargo check
```

Run the server:

```bash
cargo run
```

## Security notes

- Do not expose `LIVEKIT_API_SECRET` to React or browser code.
- Add authentication before exposing `/speak` publicly.
- Add request size limits before deploying to production.
- Restrict CORS from `*` to your real frontend origin in production.
- Add rate limiting to prevent TTS provider abuse and unexpected costs.
- Use HTTPS in production.
