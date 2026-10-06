# Captured Windows GPU verifier build evidence

These files are the unmodified output of one real offline build of the NVIDIA
GPU attestation verifier for Windows x64, from `solpbc/attestation-sdk`
revision `8fdbb0f8c10594a5f88f77fdec4766803b4e6d59`:

- `build-report.json`: the SDK build's own report, as Windows PowerShell 5.1
  wrote it (UTF-8 with a byte-order mark).
- `nvattest.exe.dumpbin.txt`: `dumpbin` headers, dependents and imports of the built
  `nvattest.exe` (SHA-256
  `220849fea69d60563fc6ef0ea7c020450d842565d08e7cbd2eecfecbd41ecdc8`).
- `regorus-Cargo.lock`: the `sol/release/regorus-Cargo.lock` member of the
  pinned source archive (`git archive` of that revision), byte for byte.
- `tool-identities.json`: the build host's native tool population. It predates
  the driver's invoked-version tool census, so its shape is not the admission
  census contract.

They are test fixtures for producer admission. They are kept byte for byte, so
do not reformat them.

The admission tests synthesize the controlled-build receipt, the validation
log, the evidence wrapper and the tool census around these captured bytes,
because no run of the controlled driver has produced them yet. The embedded
SDK report keeps the captured bytes, byte-order mark included.
