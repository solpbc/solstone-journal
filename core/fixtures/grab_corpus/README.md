# grab_corpus

Checked-in native media fixtures. Hashes and source commands are independent of the decoder under test.

- `distinct.mkv` — SHA-256 `f4e8d1aa0cee50288f8b97808dced883ad3c9af01c779f4b9a066f908f328caa` (2327 bytes). Source: `ffmpeg -y -v error -f lavfi -i testsrc2=size=32x32:rate=1:duration=3 -c:v ffv1 distinct.mkv`. Codec ffv1, container Matroska.
- `null-pts.h264` — SHA-256 `94d9948f2789b8b5543c27fd5c2a836a2b3df30541143e7f5d537ed39d923792` (798 bytes). Source: `ffmpeg -y -v error -f lavfi -i testsrc2=size=32x32:rate=1:duration=3 -c:v libopenh264 -f h264 null-pts.h264`. Codec H.264 (libopenh264), raw Annex-B elementary stream.
