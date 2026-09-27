# grab_corpus

Checked-in native media fixtures. Hashes and source commands are independent of the decoder under test.

- `distinct.mkv` — SHA-256 `41b87a9f8c2e52304e39cecca832be635eb28fd56a7ad3d4403c743402127781` (2157 bytes). Source: `ffmpeg -y -v error -f lavfi -i testsrc2=size=32x32:rate=1:duration=3 -c:v libx264 -qp 0 -bf 0 -pix_fmt yuv420p -threads 1 -fflags +bitexact -flags:v +bitexact distinct.mkv`. Codec lossless H.264 (High 4:4:4 Predictive, 4:2:0), container Matroska. Its decoded frames are identical to the earlier FFV1 source, so the RGB hashes carried over unchanged.
- `null-pts.h264` — SHA-256 `94d9948f2789b8b5543c27fd5c2a836a2b3df30541143e7f5d537ed39d923792` (798 bytes). Source: `ffmpeg -y -v error -f lavfi -i testsrc2=size=32x32:rate=1:duration=3 -c:v libopenh264 -f h264 null-pts.h264`. Codec H.264 (libopenh264), raw Annex-B elementary stream. grab reads only MOV/MP4 and Matroska/WebM containers, so it must refuse this file.
