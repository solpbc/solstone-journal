# Captured Windows GPU verifier build evidence

These files are the unmodified output of one real offline build of the NVIDIA
GPU attestation verifier for Windows x64, from `solpbc/attestation-sdk`
revision `8fdbb0f8c10594a5f88f77fdec4766803b4e6d59`:

- `build-report.json`: the SDK build's own report, as Windows PowerShell 5.1
  wrote it (UTF-8 with a byte-order mark).
- `nvattest.exe.dumpbin.txt`: `dumpbin` headers, dependents and imports of the built
  `nvattest.exe` (SHA-256
  `220849fea69d60563fc6ef0ea7c020450d842565d08e7cbd2eecfecbd41ecdc8`).
- `tool-identities.json`: the build host's native tool census.

They are test fixtures for producer admission. They are kept byte for byte, so
do not reformat them.
