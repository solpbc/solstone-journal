# Captured Windows GPU verifier build evidence

`build-report.json` and `nvattest.exe.dumpbin.txt` are the unmodified output of
one real controlled offline build of the NVIDIA GPU attestation verifier for
Windows x64 (`core/distribution/nvattest-windows-build.ps1`, 2026-10-30), from
`solpbc/attestation-sdk` revision `25ea26bc6e3cee09a344f70f29c03481420e8077`
(nvattest 1.2.2-sol.8: OpenSSL 3.6.5, xmlsec 1.2.43, curl 8.23.0):

- `build-report.json`: the SDK build's own report, as Windows PowerShell 5.1
  wrote it (UTF-8 with a byte-order mark).
- `nvattest.exe.dumpbin.txt`: `dumpbin` headers, dependents and imports of the built
  `nvattest.exe` (SHA-256
  `3cf0fdb783868a2e753b35f152dbc68a418b9697ca75ce56c905acfabafdf261`).
- `regorus-Cargo.lock`: the `sol/release/regorus-Cargo.lock` member of the
  pinned source archive (`git archive` of that revision), byte for byte.
- `tool-identities.json`: a build host's native tool population, kept from the
  earlier `8fdbb0f` qualification build. It predates the driver's
  invoked-version tool census, so its shape is not the admission census
  contract.

They are test fixtures for producer admission. They are kept byte for byte, so
do not reformat them.

The admission tests synthesize the controlled-build receipt, the validation
log, the evidence wrapper and the tool census around these captured bytes,
because the real ones bind archives too large to commit. The embedded SDK
report keeps the captured bytes, byte-order mark included.
