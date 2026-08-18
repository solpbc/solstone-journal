# parakeet-helper

Swift helper for the Parakeet v3 STT backend in solstone.

## Build

```bash
swift build -c release
```

Built binary:

```text
.build/release/parakeet-helper
```

## CLI

```text
parakeet-helper --version
parakeet-helper [--cache-dir PATH] [--model v2|v3] <audio.wav>
```

Rules:

- `--version` is standalone.
- `--cache-dir` defaults to `~/Library/Application Support/solstone/parakeet/models`.
- `--model` defaults to `v3`.
- accepted `--model` values: `v2`, `v3`.

## Stdout Schema

Success emits one UTF-8 JSON object followed by `\n`:

```json
{
  "path": "string",
  "transcript": "string",
  "confidence": 0.0,
  "audio_sec": 0.0,
  "load_ms": 0,
  "transcribe_ms": 0,
  "rtfx": 0.0,
  "token_timings": [
    {
      "token": "string",
      "token_id": 0,
      "start": 0.0,
      "end": 0.0,
      "confidence": 0.0
    }
  ],
  "model_version": "parakeet-tdt-0.6b-v3",
  "fluidaudio_version": "0.14.0",
  "hardware": "MacBookPro18,3 / Apple M4 Max",
  "macos_version": "26.4.1",
  "swift_version": "Apple Swift version 6.3.1"
}
```

`--version` emits one UTF-8 JSON object followed by `\n`:

```json
{
  "fluidaudio_version": "0.14.0",
  "model_version_default": "v3",
  "swift_version": "Apple Swift version 6.3.1",
  "hardware": "MacBookPro18,3 / Apple M4 Max",
  "macos_version": "26.4.1"
}
```

## Stderr Schema

Non-zero exits emit one UTF-8 JSON object followed by `\n`:

```json
{
  "category": "argv|cache|model_download|transcribe",
  "message": "human-readable string",
  "detail": "optional extra detail"
}
```

## Exit Codes

- `0`: success
- `2`: argv / input parsing error
- `3`: cache directory creation / write failure
- `4`: model download / load failure
- `5`: transcription failure

## Fixture Regeneration

```bash
say "The quick brown fox jumps over the lazy dog." -o /tmp/parakeet_sample.aiff
afconvert -f WAVE -d LEI16@16000 /tmp/parakeet_sample.aiff core/fixtures/parakeet_sample.wav
rm /tmp/parakeet_sample.aiff
```
