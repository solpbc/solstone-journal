# packaged nvidia reference manifests

these original nvidia-signed tcg xml documents are compiled into the native
journal payload. [the profile mapping and approved sha256 digests](../src/nvgpu/rims.rs)
are release inputs. the binary creates a private, temporary directory containing
only that profile's documents, then invokes the pinned nvattest verifier with
`--rim-store dir`. no installed rim resource directory or working-directory lookup
is needed on linux or macos.

## qualified profile

only the existing production pcr fingerprint is mapped. its driver is
`595.71.05`, with these h100 firmware variants:

- `NV_GPU_VBIOS_1010_0210_886_96009F0004`: a fresh, non-content attestation
  captured and appraised on 2026-10-01.
- `NV_GPU_VBIOS_1010_0210_886_9600880011`: the previously qualified gpu
  envelope in [the attestation fixtures](../../../../tests/fixtures/spp_attest/),
  appraised again on 2026-10-01.

the outcomes are recorded in [the qualification receipt](qualification.json).
both use `NV_GPU_DRIVER_GH100_595.71.05`. the cpu fingerprint selects the
allowed documents only after cpu verification. nvattest derives the requested
identities from the gpu report and still checks signatures, certificate chains,
versions, measurements, fresh challenge and fresh nonce-matching ocsp responses.
unknown identities have no matching local file and fail closed. adding a
manifest profile does not change the accepted cpu pins in `pins.rs`.

## provenance

retrieved unmodified from nvidia's [reference manifest api](https://docs.nvidia.com/attestation/cloud-services/latest/rim/rim_api.html)
on 2026-10-01. each base64-decoded document's sha256 matched the api response
and is pinned in the profile mapping. the signed bytes are preserved, including
their nvidia certificate chains. filename and api identifier alone are not
signed bindings to the cpu image; the finite mapping and approved digests are
journal release policy, with the vendor signature and measurement checks
performed separately on every appraisal.

api retrieval paths:

- [driver](https://rim.attestation.nvidia.com/v1/rim/NV_GPU_DRIVER_GH100_595.71.05)
- [current firmware](https://rim.attestation.nvidia.com/v1/rim/NV_GPU_VBIOS_1010_0210_886_96009F0004)
- [previously qualified firmware](https://rim.attestation.nvidia.com/v1/rim/NV_GPU_VBIOS_1010_0210_886_9600880011)

nvidia remains the signing and revocation authority. this bundle removes rim
fetching from appraisal; ocsp network checks remain required. an unknown
firmware variant needs qualification and a new journal bundle before admission.
