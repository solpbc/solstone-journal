# Captured Windows GPU verifier build evidence

These files are the unmodified output of one real offline build of the NVIDIA
GPU attestation verifier for Windows x64, from `solpbc/attestation-sdk`
revision `8fdbb0f8c10594a5f88f77fdec4766803b4e6d59`:

- `build-report.json`, `nvattest.exe.dumpbin.txt`, and `tool-identities.json` are captured bytes.
- `tool-identities.json` is the old tool census and is not the admission census.
- A receipt, validation log, and evidence wrapper are synthesized because no driver run exists yet.

They are test fixtures for producer admission. They are kept byte for byte, so
do not reformat them.

