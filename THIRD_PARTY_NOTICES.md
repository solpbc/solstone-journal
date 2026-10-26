# third-party notices

this file records third-party materials used by solstone, including model
weights, tokenizer data and place names bundled with solstone, place data an owner may download for the map app, the map app's renderer, the
restic, rclone, nvattest and CED components bundled in every package, provider artifacts downloaded at runtime into the
journal provider cache, and test fixtures derived from permissively licensed
sources.

## FFmpeg

The journal statically links FFmpeg for media import and processing. Its
LGPL-2.1-or-later licence text, copyright information, pinned source identity
and source/relink location accompany each journal payload under
`share/licenses/ffmpeg/`. The pinned source is FFmpeg commit
`946fcce07b6dcd0331c8cc609192aeff5e1924f8`; see
`share/licenses/ffmpeg/SOURCE-NOTICE.md` for its archive digest and source URL.

## PDF extraction engine

The journal bundles a native PDFium library from the pinned
`bblanchon/pdfium-binaries` release `chromium/7920`. Its licence and
third-party texts accompany the library under `lib/solstone-core-pdf/`.

### PDFium

Attribution: Google PDFium project.

Source:

- Project: https://pdfium.googlesource.com/pdfium/
- Binary release: https://github.com/bblanchon/pdfium-binaries/releases/tag/chromium%2F7920

License notice: BSD 3-Clause License (BSD-3-Clause).

## bundled model weights

| Bundled file | Upstream model | Source artifact | License | SHA-256 |
|---|---|---|---|---|
| `lib/solstone_journal_models/assets/wespeaker-resnet34-256.onnx` | WeSpeaker ResNet34 speaker embedding model trained on VoxCeleb | `wespeaker_en_voxceleb_resnet34.onnx` from the k2-fsa/sherpa-onnx `speaker-recongition-models` release | CC-BY-4.0 | `5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94` |
| `lib/solstone_journal_models/assets/pyannote-segmentation-3.0.onnx` | `pyannote/segmentation-3.0` speaker segmentation model | `onnx/model.onnx` from `onnx-community/pyannote-segmentation-3.0` | MIT | `057ee564753071c0b09b5b611648b50ac188d50846bff5f01e9f7bbf1591ea25` |
| `lib/solstone_journal_models/assets/firered_vad.onnx` | FireRedVAD offline voice activity detection model | `pretrained_models/onnx_models/fireredvad_vad.onnx` from `FireRedTeam/FireRedVAD` at `c30ec49e8cc69642b0ee65362eba11b9d11c6e54` | Apache-2.0 | `a226e68e695548754a9ab5480a55bbcb8522aa69111cb97e4f093ddce63ad5dc` |
| Embedded `firered_vad_cmvn.ark` normalization statistics | FireRedVAD global normalization statistics, compiled into the VAD helper and mobile library | `pretrained_models/onnx_models/cmvn.ark` from the same FireRedVAD revision | Apache-2.0 | `c87f6f13edf0f0ec7535ddfc9cc3387d9268cb234b70182d566c5e2edf3ca473` |

FireRedVAD attribution and source details ship in `share/licenses/models/NOTICE.md`. Its original Apache License 2.0 text ships as `share/licenses/models/FireRedVAD-LICENSE.txt`.

## bundled place names (area tier)

`lib/solstone_journal_models/assets/geo/area-tier.bin` (SHA-256
`47208568024e0937b7fd130d82c7557b761a1920bbbb2d0489fe2f4645d0f1ae`) names a
location reading after a nearby town, and gives the map app its country and
state borders. The journal reads it on the owner's own computer; nothing is
looked up anywhere. It is built by `scripts/build_location_area_tier.py` from
the two sources below.

### GeoNames

Attribution: GeoNames, https://www.geonames.org/

Source: `cities500.zip`, `admin1CodesASCII.txt` and `countryInfo.txt` from
https://download.geonames.org/export/dump/

License notice: Creative Commons Attribution 4.0 International (CC-BY-4.0).
License text: https://creativecommons.org/licenses/by/4.0/legalcode.txt

Changes: sections of populated places and historical, abandoned or destroyed
places are left out; each place keeps only its name, ASCII name, position
rounded to 1e-5 degrees, population, country and first-level division name;
the rows are re-encoded into the file's compact binary form. The map app
credits GeoNames, with a link, wherever it draws the map.

### Natural Earth

Attribution: Natural Earth, https://www.naturalearthdata.com/

Source: `ne_10m_admin_0_countries` and `ne_10m_admin_1_states_provinces`
(GeoJSON) from https://github.com/nvkelso/natural-earth-vector at commit
`ca96624a56bd078437bca8184e78163e5039ad19`.

License notice: public domain. Natural Earth's terms of use:
https://www.naturalearthdata.com/about/terms-of-use/

Changes: outlines simplified (state and province borders to 0.01 degrees,
country borders to 0.02 degrees) and keyed by name and ISO country code.

## downloaded place data (location accuracy)

An owner who chooses "location accuracy" in the map app downloads two files,
version `overture-2026-09-23.1-r2`, from the update origin into
`cache/location/accuracy/` in their journal: `venues.slpk` (SHA-256
`7bac3fd446c7d17c0c2811ccba04579b5210c69afccabaf046075437a901316e`) and
`containment.slpk` (SHA-256
`0f8f4c7270c5938ebd395e1a879a9b23ce265db10af952a0af1f79682f319063`). The
journal reads them on the owner's own computer to name the places within a
stop's margin of error and the outlines a position lies inside. Both are built
by `scripts/build_location_accuracy_packs.py` from Overture Maps release
`2026-09-23.1`, and each carries its notices in its own header. The two files
are separate databases; the journal looks a position up in each on its own.

### Overture Maps Places

Attribution: Overture Maps Foundation, https://overturemaps.org; see
https://docs.overturemaps.org/attribution/

License notice: per source record, CDLA-Permissive-2.0
(https://cdla.dev/permissive-2-0/), Apache-2.0
(https://www.apache.org/licenses/LICENSE-2.0) or CC0-1.0
(https://creativecommons.org/publicdomain/zero/1.0/).

Changes: places are filtered (confidence of at least 0.5, not permanently
closed, at least one source other than a business registry) and reduced to
name, category, confidence and position, re-encoded into the file's compact
form.

The full texts of CDLA-Permissive-2.0 and Apache-2.0 are in
`licenses/CDLA-Permissive-2.0.txt` and `licenses/Apache-2.0.txt`, and travel in
the file's header.

Foursquare's notice, in full:

```text
© 2025 Foursquare Labs, Inc. All rights reserved. 

The Foursquare OS Places dataset (the “Data”) is licensed under the Apache License, Version 2.0 (the "License"). You may not use, modify, or distribute the Data except in compliance with the License. 
 
As set forth more fully in the License, if you use, modify, or distribute the Data, you must:
* provide recipients with a copy of the License.
* if applicable, include prominent notices to the extent you’ve changed the Data.
* preserve attribution to Foursquare, including preserving the full content of this NOTICE.txt file.
 
To ensure appropriate attribution to Foursquare, we recommend the following:
* if using/distributing the Data in flat file form as-is or after making changes/modifications: include this NOTICE.txt file, which may be modified to include an additional notice of your changes/modifications, if any. 
* if using/distributing the Data in API form as-is or after making changes/modifications: include a copy of the content from this NOTICE.txt file prominently in your developer documentation for such API, which may be modified to include an additional notice of your changes/modifications, if any. 
 
You may obtain a copy of the License at: http://www.apache.org/licenses/LICENSE-2.0. Unless required by applicable law or agreed to in writing, the Data distributed under the License is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.

See the License for the specific language governing permissions and limitations under the License.

We also encourage you to join our Placemaker community (https://opensource.foursquare.com/placemaker/)—where you can contribute and provide suggestions to improve the accuracy of the Data for future releases for yourself and others.
```

### Overture Maps outlines (`containment.slpk`)

Attribution: Outlines © OpenStreetMap contributors and Overture Maps
Foundation, available under the Open Database License
(https://opendatacommons.org/licenses/odbl/1-0/); see
https://docs.overturemaps.org/attribution/

License notice: this database is made available under the Open Database
License: https://opendatacommons.org/licenses/odbl/1-0/. Rights in individual
contents stay with their sources, under the licences listed at
https://docs.overturemaps.org/attribution/. Some outlines are adapted from
sources available under the Creative Commons Attribution 4.0 International
licence (CC BY 4.0, https://creativecommons.org/licenses/by/4.0/):
geoBoundaries; Esri Community Maps contributors; Land Information New Zealand
(LINZ); Google Open Buildings; Qian Shi et al., building outlines for East
Asian countries (doi:10.5281/zenodo.8174931); and work derived from BTN 2024,
ign.es.

Changes: named outlines are selected from Overture Maps divisions, land use,
airports and buildings, simplified (tolerance per layer in the file's
`tolerance_m`) and re-encoded into the file's compact form by
`scripts/build_location_accuracy_packs.py`, which is the build method.

## map renderer (Leaflet)

The map app draws with Leaflet 1.9.4, served from the journal at
`/static/vendor/leaflet/`, with no tile layer and no request off the journal.

Source: https://registry.npmjs.org/leaflet/-/leaflet-1.9.4.tgz (SHA-256
`84c65a256e50657896f54c33bd857b6849ebe94c817803be818bf32a3dde0b77`);
project https://leafletjs.com

License notice: BSD 2-Clause License (BSD-2-Clause). The licence text ships
beside the library as `vendor/leaflet/LICENSE`.

## Windows time zone names (Unicode CLDR)

Calendar files exported from Outlook name time zones the Windows way, such as
"Pacific Standard Time". The journal reads them through the world mappings of
the Unicode CLDR `windowsZones` table, kept as data in
`core/crates/solstone-core-import-sources/src/windows_zones.tsv` and compiled
into the journal.

Source: `common/supplemental/windowsZones.xml` from
https://github.com/unicode-org/cldr at revision
`a70bfe93a2c74473baaedf01eea43b027e782979`.

Changes: only the world ("001") mappings are kept, one Windows name and its
IANA zone per line, each zone written as its current IANA name from CLDR's
`common/bcp47/timezone.xml` at the same revision.

License notice: Unicode License v3 (Unicode-3.0), Copyright © Unicode, Inc.
The full copyright and permission notice is in `licenses/Unicode-3.0.txt`.

## token counter (Qwen3.5-4B tokenizer merges)

The journal counts every thinking input in the Qwen3.5-4B tokenizer, so a
request is sized the same way whatever thinks. Counting uses the tokenizer's
ranked merges, kept in `core/crates/solstone-core-tokens/assets/qwen3.5-4b-merges.txt.gz`
(SHA-256 `f4e4edf3d11f42411c988269c90bdc7589314745eec559ae32fcac986c7a6b4f`) and compiled into the journal.

Source: `tokenizer.json` from https://huggingface.co/Qwen/Qwen3.5-4B at revision
`851bf6e806efd8d0a36b00ddf55e13ccb7b8cd0a`.

Changes: only the merges are kept, one per line in rank order, gzip-compressed,
by `scripts/build_qwen_token_merges.py`.

License notice: Apache License 2.0 (Apache-2.0), Copyright 2026 Alibaba Cloud.
The full license text is in `licenses/Apache-2.0.txt` and, with the attribution,
in `share/licenses/models/NOTICE.md`.

## bundled test fixtures

### Paradigm Shift AI delayed-video regression fixture

`core/crates/solstone-core-describe/tests/fixtures/delayed_video_probe_screen.mp4`
is a 9.4-second stream-copy subset of source item
`cmcc8u6yc00va1p1ydsdu52zy` from the Computer Use Dataset by Paradigm Shift AI.
the source is `journal/20260201/field.screen/094500_300/screen.mp4` at
`solpbc/field_journal` commit `c1edcc8909f907075916e9ad0f63701da7b607b5`
(SHA-256 `09fa691b99e4d0450922ae39d5be9c16231dd2fa00328d6057c5e0e92e4df6d1`).
the committed subset has SHA-256
`091fa2d732148a0c1e611a72bd320d9db200a790a0fcdf17cfc83d7280d2c17d`.

source:

- Dataset: https://huggingface.co/datasets/anaisleila/computer-use-data-psai
- Provider: Paradigm Shift AI

license notice: MIT License.

Copyright (c) 2025 Paradigm Shift AI
Anais Howland, Ashwin Thinnappan, Jameel Shahid Mohammed

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## MLX (macos local inference helper)

macos packages include `mlx-helper` and its `mlx-swift_Cmlx.bundle` Metal
resources. The helper compiles the Swift packages below at the revisions
listed in its accompanying `Package.resolved`.

| package | version or revision |
|---|---|
| [async-http-client](https://github.com/swift-server/async-http-client/tree/4c005f955e83f888d5616e579717a36d2dfc6301) | `1.36.2` |
| [eventsource](https://github.com/mattt/EventSource/tree/86b5096ac59ab46e66bd1f6377c604bc1dab0bc2) | `1.5.1` |
| [hummingbird](https://github.com/hummingbird-project/hummingbird/tree/e12178302dacb8226fc78157f89078a59c75dd98) | `2.28.0` |
| [mlx-swift](https://github.com/ml-explore/mlx-swift/tree/19601207e9a0de51e03ee6ec0c3c5f3784275075) | `0.32.3` |
| [mlx-swift-lm](https://github.com/ml-explore/mlx-swift-lm/tree/48114eedf3c3d4a682a6b1d644cc5af5c2945d10) | `48114eedf3c3` |
| [swift-algorithms](https://github.com/apple/swift-algorithms/tree/87e50f483c54e6efd60e885f7f5aa946cee68023) | `1.2.1` |
| [swift-asn1](https://github.com/apple/swift-asn1/tree/3b6410f7dee09eb33cdd26260c5fd47fda19b0e2) | `1.7.3` |
| [swift-async-algorithms](https://github.com/apple/swift-async-algorithms/tree/13713a4ffdee8abd929f92568ee9462a46ae26e0) | `1.1.7` |
| [swift-atomics](https://github.com/apple/swift-atomics/tree/0442cb5a3f98ab802acb777929fdb446bda11a34) | `1.3.1` |
| [swift-certificates](https://github.com/apple/swift-certificates/tree/ff86b924ead66f853b8baf91f3c41926a8f36177) | `1.21.0` |
| [swift-collections](https://github.com/apple/swift-collections/tree/35a1edcb8ebd2bfe251d587f0b69e62af28f508c) | `1.7.3` |
| [swift-configuration](https://github.com/apple/swift-configuration/tree/4e916072eb34f3c136ae64bd2859d6072cdb1b6e) | `1.3.0` |
| [swift-crypto](https://github.com/apple/swift-crypto/tree/da9d28d69ebe3894b18376c8f2395c2f37b8448f) | `4.5.2` |
| [swift-distributed-tracing](https://github.com/apple/swift-distributed-tracing/tree/297b5b9c82a5631e3199ce3ec0993a3cfb0fbb84) | `1.7.2` |
| [swift-http-structured-headers](https://github.com/apple/swift-http-structured-headers/tree/933538faa42c432d385f02e07df0ace7c5ecfc47) | `1.7.0` |
| [swift-http-types](https://github.com/apple/swift-http-types/tree/bff4b6903cdc99dda49649dd52f46c11cfd3ed50) | `1.8.0` |
| [swift-huggingface](https://github.com/huggingface/swift-huggingface/tree/c30143a4616f3fa994ee3ab3e3464f752845c07c) | `1.0.0` |
| [swift-jinja](https://github.com/huggingface/swift-jinja/tree/1e4d0dc69f7ec1277131fb3853808229c5991825) | `2.5.2` |
| [swift-log](https://github.com/apple/swift-log/tree/c9c6bb9d275ec0d95c3ad16391f3f045dc623104) | `1.16.2` |
| [swift-metrics](https://github.com/apple/swift-metrics/tree/9467f9cc3c32b4386fad1ecc4618328887ef5758) | `2.12.0` |
| [swift-nio](https://github.com/apple/swift-nio/tree/6db9d9abd92ad7399b8a564cafbb87d3507245fa) | `2.104.0` |
| [swift-nio-extras](https://github.com/apple/swift-nio-extras/tree/41449336c8ecfadac6b4b5be75f9c3c306e61ced) | `1.35.1` |
| [swift-nio-http2](https://github.com/apple/swift-nio-http2/tree/0f3e54e29c944c2e835ad52159da7d9e1c94ac69) | `1.46.0` |
| [swift-nio-ssl](https://github.com/apple/swift-nio-ssl/tree/322f3c2a4a21df31c84ca416bf65ee5e9059e440) | `2.37.5` |
| [swift-nio-transport-services](https://github.com/apple/swift-nio-transport-services/tree/67787bb645a5e67d2edcdfbe48a216cc549222d5) | `1.28.0` |
| [swift-numerics](https://github.com/apple/swift-numerics/tree/0c0290ff6b24942dadb83a929ffaaa1481df04a2) | `1.1.1` |
| [swift-service-context](https://github.com/apple/swift-service-context/tree/853ba3d9ce78851b9cc20108e8bd4f13f7fa546c) | `1.5.1` |
| [swift-service-lifecycle](https://github.com/swift-server/swift-service-lifecycle/tree/a18ee16b8039866fd919bdb4e8e906882d4ecc75) | `2.13.0` |
| [swift-syntax](https://github.com/swiftlang/swift-syntax/tree/79e4b74a295b6eb74a8b585e3a39d29e70c1dbd1) | `603.0.2` |
| [swift-system](https://github.com/apple/swift-system/tree/869129b7bf4ecc57b97d0193ad29690ca2134750) | `1.8.1` |
| [swift-transformers](https://github.com/huggingface/swift-transformers/tree/48473106a93b2d8dfb114b82f0628ebbb71734e7) | `1.3.5` |
| [yyjson](https://github.com/ibireme/yyjson/tree/8b4a38dc994a110abaec8a400615567bd996105f) | `0.12.0` |

`mlx-swift`, `mlx-swift-lm`, EventSource and yyjson use the MIT license. Hummingbird and
the other Swift packages above use Apache-2.0; Swift packages may include
the Swift runtime library exception in their license text.

The helper includes a modified guided-generation loop from `mlx-swift-lm`
under its MIT license. The changes batch forced tokens, preserve sampling
controls and apply packed grammar masks. MLX also includes fmt and
nlohmann/json (MIT), and Apple's metal-cpp (Apache-2.0). XGrammar is
Apache-2.0. SwiftNIO and SwiftCrypto include their own dependency notices;
the BoringSSL license texts accompany those packages.

macos packages carry the license and notice texts under
`share/licenses/mlx-helper/`, preserving their package and source paths.
The modified loop's MIT notice is in `guided-generation/LICENSE` there.
The model's separate attribution is in the local-model section below.

The package also includes Swift's Span compatibility library from the build
toolchain, under Apache-2.0 with the Swift runtime library exception. Its
license is in `swift-runtime/LICENSE.txt` with the helper notices.

## FluidAudio (macos speech helper)

The macos journal's speech helper (`bin/parakeet-helper`) compiles in
FluidAudio 0.17.7 by FluidInference
(https://github.com/FluidInference/FluidAudio/tree/v0.17.7, commit
`503b4bd1bbf7220882de39fe8ae6716aae4132da`), under the Apache License 2.0.
The helper uses sol pbc's leading-gap repair backport, version `0.17.7+sol.1`,
pinned at `03e48b07ab136062987f157b6ed84ebc8f5eb204` in
https://github.com/quartzjer/FluidAudio. The patch retries missing opening speech;
it does not change the model files or license texts.
FluidAudio includes code adapted from fastcluster (BSD-2-Clause), VBx
(Apache-2.0) and a Japanese text frontend drawn from MIT, Apache-2.0 and BSD
sources. The helper is built without FluidAudio's optional NeMo
text-processing library.

macos packages carry FluidAudio's `LICENSE` and its `fastcluster-LICENSE.md`,
`vbx-LICENSE.md` and `JapaneseG2P-LICENSE.md`, unchanged from the tagged
source, in `share/licenses/fluidaudio/`.

## provider artifacts (parakeet-cpp)

On linux, sol pbc redistributes these artifacts from `updates.solstone.app` on
demand into the journal provider cache when an owner installs parakeet.cpp. On
windows, the model and sol pbc's own build of the server ship inside the signed
journal package. They are not bundled in this repository.

### parakeet.cpp server binary

Attribution: parakeet.cpp project (mudler).

Source:

- linux release binaries: https://github.com/mudler/parakeet.cpp/releases/tag/v0.6.1
- windows build: sol pbc builds v0.5.0 from source with one patch
  (`core/distribution/parakeet-windows-patches/`). It statically links ggml
  (MIT, https://github.com/ggml-org/ggml), whose license ships with the package.
- Project: https://github.com/mudler/parakeet.cpp

License notice: MIT.

The MIT license permits sol pbc's redistribution of this server binary and its
windows build.

The linux v0.6.1 archives have notice sidecars beside the downloads:

- [parakeet-v0.6.1-bin-linux-cpu-arm64.tar.gz](https://updates.solstone.app/assets/parakeet-server/v0.6.1/parakeet-v0.6.1-bin-linux-cpu-arm64.tar.gz) · [full notices](https://updates.solstone.app/assets/parakeet-server/v0.6.1/parakeet-v0.6.1-bin-linux-cpu-arm64.tar.gz.THIRD_PARTY_NOTICES.md)
- [parakeet-v0.6.1-bin-linux-cpu-x64.tar.gz](https://updates.solstone.app/assets/parakeet-server/v0.6.1/parakeet-v0.6.1-bin-linux-cpu-x64.tar.gz) · [full notices](https://updates.solstone.app/assets/parakeet-server/v0.6.1/parakeet-v0.6.1-bin-linux-cpu-x64.tar.gz.THIRD_PARTY_NOTICES.md)
- [parakeet-v0.6.1-bin-linux-vulkan-arm64.tar.gz](https://updates.solstone.app/assets/parakeet-server/v0.6.1/parakeet-v0.6.1-bin-linux-vulkan-arm64.tar.gz) · [full notices](https://updates.solstone.app/assets/parakeet-server/v0.6.1/parakeet-v0.6.1-bin-linux-vulkan-arm64.tar.gz.THIRD_PARTY_NOTICES.md)
- [parakeet-v0.6.1-bin-linux-vulkan-x64.tar.gz](https://updates.solstone.app/assets/parakeet-server/v0.6.1/parakeet-v0.6.1-bin-linux-vulkan-x64.tar.gz) · [full notices](https://updates.solstone.app/assets/parakeet-server/v0.6.1/parakeet-v0.6.1-bin-linux-vulkan-x64.tar.gz.THIRD_PARTY_NOTICES.md)

windows packages built from this source place the YaRN and cpp-httplib blocks at `share/licenses/parakeet/YaRN-LICENSE.txt` and `share/licenses/parakeet/cpp-httplib-LICENSE.txt` beside the parakeet.cpp and ggml licence files. The full text for each component follows.

## parakeet.cpp (linux CLI and server; windows server)

Sources:

- linux: https://github.com/mudler/parakeet.cpp/blob/5c9b0fbbd8e611d88e47d7e1ebae9bca9b447e8f/LICENSE
- windows: https://github.com/mudler/parakeet.cpp/blob/1bfbebfaaf493866f49597cd3b7901959d395c60/LICENSE

```text
MIT License

Copyright (c) 2026 the parakeet.cpp authors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## ggml (linux CLI and server; windows server)

Source: https://github.com/ggml-org/ggml/blob/e705c5fed490514458bdd2eaddc43bd098fcce9b/LICENSE

```text
MIT License

Copyright (c) 2023-2026 The ggml authors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Mozilla Foundation sgemm.cpp (linux CLI and server)

Source: https://github.com/ggml-org/ggml/blob/e705c5fed490514458bdd2eaddc43bd098fcce9b/src/ggml-cpu/llamafile/sgemm.cpp

```text
Copyright 2024 Mozilla Foundation

Permission is hereby granted, free of charge, to any person obtaining
a copy of this software and associated documentation files (the
"Software"), to deal in the Software without restriction, including
without limitation the rights to use, copy, modify, merge, publish,
distribute, sublicense, and/or sell copies of the Software, and to
permit persons to whom the Software is furnished to do so, subject to
the following conditions:

The above copyright notice and this permission notice shall be
included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS
BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN
ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## YaRN (linux CLI and server; windows server)

Source: https://github.com/jquesnelle/yarn/blob/995db5b575e75230b3384d658f8b944c9662f775/LICENSE

```text
MIT License

Copyright (c) 2023 Jeffrey Quesnelle and Bowen Peng

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## cpp-httplib v0.18.3 (linux and windows server only)

Source: https://github.com/yhirose/cpp-httplib/blob/v0.18.3/LICENSE

```text
The MIT License (MIT)

Copyright (c) 2017 yhirose
Copyright (c) 2024 Yuji Hirose. All rights reserved.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## ced.cpp (linux CLI and server)

Source: https://github.com/localai-org/ced.cpp/blob/736a4ee46a31d4ff38b41add65ce0f2cf1aaa05f/LICENSE

```text
MIT License

Copyright (c) 2026 the ced.cpp authors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## voice-detect.cpp (linux CLI)

Source: https://github.com/localai-org/voice-detect.cpp/blob/bca46bcbc2fe68169c2d7c414e9b290a7cb89911/LICENSE

```text
MIT License

Copyright (c) 2026 the voice-detect.cpp authors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## parakeet TDT 0.6B v3 GGUF model

Attribution: parakeet-cpp-gguf (mudler), NVIDIA NeMo Parakeet TDT 0.6B v3.

Source:

- Model repository: https://huggingface.co/mudler/parakeet-cpp-gguf
- Pinned revision: bf0af9f425fa01809cadec671b3cb672709d13e9
- Downloaded file: tdt-0.6b-v3-q8_0.gguf

License notice: Creative Commons Attribution 4.0 International (CC-BY-4.0).
License text: https://creativecommons.org/licenses/by/4.0/legalcode.txt

sol pbc redistributes this model under CC-BY-4.0. The attribution above and
the CC-BY-4.0 license URI accompany the redistributed copy.

## runtime-downloaded provider artifacts (Parakeet Core ML)

sol pbc redistributes these artifacts from `updates.solstone.app` on demand
into the journal provider cache when an owner installs the Core ML Parakeet
transcription backend. They are not bundled in this repository.

### Parakeet TDT 0.6B v3 Core ML conversion

Attribution: `nvidia/parakeet-tdt-0.6b-v3`.

Source:

- Model repository: https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3
- Pinned revision: `aed02740059203c4a87495924f685de3722ae9ce`

License notice: Creative Commons Attribution 4.0 International (CC-BY-4.0).
License text: https://creativecommons.org/licenses/by/4.0/legalcode.txt

The shipped Core ML artifacts are a modified conversion of the source model.
sol pbc redistributes this conversion under CC-BY-4.0 with the attribution,
license URI, and modification indication above.

## runtime-downloaded provider artifacts (local model)

sol pbc redistributes these artifacts from `updates.solstone.app` on demand
into the journal provider cache when an owner installs the local inference
provider. They are not bundled in this repository.

### Qwen3.5-4B GGUF model

Attribution: `unsloth/Qwen3.5-4B-GGUF`, based on Qwen3.5-4B.

Source:

- Model repository: https://huggingface.co/unsloth/Qwen3.5-4B-GGUF
- Downloaded file: `Qwen3.5-4B-Q4_K_M.gguf`
- SHA-256: `00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4`
- Downloaded file: `mmproj-F16.gguf`
- SHA-256: `cd88edcf8d031894960bb0c9c5b9b7e1fea6ebee02b9f7ce925a00d12891f864`

License notice: Apache License 2.0 (Apache-2.0).

Apache-2.0 permits sol pbc's redistribution of these model files with its
required notice and license terms.

### Qwen3.5-4B MLX 4-bit model

Attribution: `mlx-community/Qwen3.5-4B-MLX-4bit`, a 4-bit MLX conversion
of `Qwen/Qwen3.5-4B` (Alibaba Cloud).

Source:

- Model repository: https://huggingface.co/mlx-community/Qwen3.5-4B-MLX-4bit
- Pinned revision: `32f3e8ecf65426fc3306969496342d504bfa13f3`
- Base model license:
  https://huggingface.co/Qwen/Qwen3.5-4B/blob/851bf6e806efd8d0a36b00ddf55e13ccb7b8cd0a/LICENSE

License notice: Apache License 2.0 (Apache-2.0).
Copyright 2026 Alibaba Cloud.

The conversion's pinned revision has no `LICENSE` file. Its model card
identifies `Qwen/Qwen3.5-4B` as the base model. The base model's pinned
`LICENSE` supplies the Apache-2.0 terms; a copy accompanies the mirrored
model files as `LICENSE`.

## bundled sound-tag engine and model (ced.cpp)

The journal packages include the ced.cpp sound-tag engine and the ced-tiny
model it runs. On linux and macos they are at `lib/solstone-ced/`; on windows
the engine is `bin/ced.dll` and the model is under
`lib/solstone_journal_models/assets/ced/`. They are not part of solstone and
are not covered by solstone's AGPL-3.0-only license.

### ced.cpp v0.1.0 engine

Attribution: ced.cpp project (localai-org). It includes ggml.

Source:

- linux and macos: the release libraries at
  https://github.com/localai-org/ced.cpp/releases/tag/v0.1.0
  (`ced-v0.1.0-lib-linux-cpu-x64.tar.gz` SHA-256
  `915e0573bc4e17197a7a893d0eb98e1a851abb64451b2e1a8ad51f5f99040360`,
  `ced-v0.1.0-lib-linux-cpu-arm64.tar.gz` SHA-256
  `a87de0a8b086429aa5d6544a6f881a70e62726d07901734640ac85dbf146181e`,
  `ced-v0.1.0-lib-macos-metal-arm64.tar.gz` SHA-256
  `4c913ba0ece1d06ba2210da9fcaee3d8199ca3c62697c331810f224444e4054b`).
- windows: built from source at ced.cpp commit
  c04ac14b7992d00584d9e812c9bb6268598a6ce7 with ggml commit
  e705c5fed490514458bdd2eaddc43bd098fcce9b.

License notice: MIT, for ced.cpp and for ggml. The texts are at
`share/solstone-journal/licenses/ced-engine/` (linux and macos) and
`share/licenses/ced/` (windows).

### ced-tiny-q8_0 GGUF model

Attribution: `mudler/ced-gguf`, a GGUF quantization of `mispeech/ced-tiny`
(CED, Xiaomi).

Source:

- Model repository: https://huggingface.co/mudler/ced-gguf
- Pinned revision: b5e9a4aad6438763c8da16079d77563fbed35c65
- File: `ced-tiny-q8_0.gguf`
- SHA-256: `48bee4e2fc3cc85d7806e03471db24e77fda6c2a2e81ffe9ef67caebaf2bd674`

License notice: Apache License 2.0 (Apache-2.0). The text is
`ced-tiny-model-LICENSE`, beside the engine licenses.

## runtime-downloaded provider artifacts (rerank cross-encoder)

sol pbc retains these artifacts in the download catalog as a dormant pin. They
are not fetched on POSIX and are not consumed by any product path. They are not
bundled in this repository.

### rerank cross-encoder ONNX model

Attribution: `Xenova/ms-marco-MiniLM-L-6-v2`, an ONNX export of
`cross-encoder/ms-marco-MiniLM-L-6-v2`.

Source:

- Model repository: https://huggingface.co/Xenova/ms-marco-MiniLM-L-6-v2
- Pinned revision: a09144355adeed5f58c8ed011d209bf8ee5a1fec
- Downloaded files: `onnx/model.onnx`, `tokenizer.json`

License notice: Apache License 2.0 (Apache-2.0).

Apache-2.0 permits sol pbc's redistribution of these model files with its
required notice and license terms.

## bundled RF-DETR object-detection assets (rf-detr.cpp)

sol pbc bundles these artifacts in every journal release and installs them from
the release tree into the journal provider cache for local object detection.
They are not fetched at install time.

### rf-detr.cpp v0.1.0-solpbc.5 engine

Attribution: rf-detr.cpp (Ettore Di Giacinto / mudler); binaries CI-built and
released by sol pbc.

Source:

- Release binaries: https://github.com/solpbc/rf-detr.cpp/releases/tag/v0.1.0-solpbc.5
- Project: https://github.com/localai-org/rf-detr.cpp
- Pinned engine ref: ec73712e
- Release tag: v0.1.0-solpbc.5
- Bundled file: `rfdetr-v0.1.0-solpbc.5-bin-linux-cpu-x64.tar.gz`
- SHA-256: `56231d6675395ed790dba882e0335e4c79616427af558b1820975951cd9d14a7`
- Bundled file: `rfdetr-v0.1.0-solpbc.5-bin-linux-cpu-arm64.tar.gz`
- SHA-256: `2c11e1af6986571d4d9f4d2cf377018973095b10c234a9da40a3edf45cf11f9d`
- Bundled file: `rfdetr-v0.1.0-solpbc.5-bin-macos-metal-arm64.tar.gz`
- SHA-256: `46b497950c7a73000007abdb9ef54bc8b46ba0a46dcf26f6c0ae51fccd21ad71`

License notice: Apache License 2.0 (Apache-2.0).

Apache-2.0 permits sol pbc's redistribution of these engine binaries with their
required notice and license terms.

### RF-DETR nano GGUF model weights

Attribution: RF-DETR (Roboflow); GGUF conversion mudler/rfdetr-cpp-nano.

Source:

- Model repository: https://huggingface.co/mudler/rfdetr-cpp-nano
- Pinned revision: c3dc0c037df499f5503545247df6618415fca643
- Bundled file: `rfdetr-nano-f16.gguf`
- SHA-256: `d798cc448faa53209b88fc905c91beb1dd104634b95f6948cc4877540a8fd3ee`

License notice: Apache License 2.0 (Apache-2.0).

Apache-2.0 permits sol pbc's redistribution of these model weights with its
required notice and license terms.

## WeSpeaker ResNet34 / VoxCeleb

Attribution: WeSpeaker project, ResNet34 speaker embedding model trained on
VoxCeleb.

Source:

- Exact bundled artifact:
  https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/wespeaker_en_voxceleb_resnet34.onnx
- Release checksum file:
  https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/checksum.txt
- WeSpeaker project:
  https://github.com/wenet-e2e/wespeaker
- WeSpeaker pretrained-model license note:
  https://github.com/wenet-e2e/wespeaker/blob/master/docs/pretrained.md#model-license

License notice: Creative Commons Attribution 4.0 International (CC-BY-4.0).
WeSpeaker's pretrained-model documentation states that pretrained models follow
the license of the corresponding dataset, and that pretrained models on VoxCeleb
follow Creative Commons Attribution 4.0 International because VoxCeleb uses that
license. License text: https://creativecommons.org/licenses/by/4.0/legalcode.txt

## pyannote segmentation 3.0

Attribution: pyannote.audio project, `pyannote/segmentation-3.0` speaker
segmentation model.

Source:

- Exact bundled ONNX artifact:
  https://huggingface.co/onnx-community/pyannote-segmentation-3.0/resolve/main/onnx/model.onnx
- ONNX-community model card:
  https://huggingface.co/onnx-community/pyannote-segmentation-3.0
- Original pyannote model card:
  https://huggingface.co/pyannote/segmentation-3.0
- pyannote.audio source:
  https://github.com/pyannote/pyannote-audio

License notice: MIT. The retained MIT notice follows.

```text
MIT License

Copyright (c) 2020 CNRS

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## runtime-downloaded provider artifacts (llama.cpp CUDA)

These artifacts are downloaded on demand into the journal provider cache for
solstone's local inference runtime on supported NVIDIA GPU systems. They are
distributed as application components, not as a stand-alone CUDA distribution.
sol pbc redistributes them from `updates.solstone.app`; they are not bundled in
this repository.

### llama.cpp and ggml runtime

Files: `llama-server`, `libllama-server-impl.so`,
`libllama-common.so.0`, `libmtmd.so.0`, `libllama.so.0`,
`libggml.so.0`, `libggml-base.so.0`, `libggml-cuda.so`, and the
architecture-specific `libggml-cpu-*.so` files.

Source: https://github.com/ggml-org/llama.cpp

License: MIT License.

The complete llama.cpp MIT license and copyright notice is reproduced in
`licenses/llama.cpp-LICENSE.txt` and accompanies each runtime artifact.
The MIT license permits sol pbc's redistribution of these llama.cpp runtime
files.

### NVIDIA CUDA runtime components

Files: `libcudart.so.13`, `libcublas.so.13`,
`libcublasLt.so.13`.

Source:
- linux x86_64 archive `llama-b11669-bin-linux-cuda13-amd64-sol1.tar.gz` and linux arm64 archive `llama-b11669-bin-linux-cuda13-arm64-sol1.tar.gz` each carry CUDA Toolkit 13.4.1 components. Each archive is packaged with the MIT license file `licenses/llama.cpp-LICENSE.txt` and the unmodified EULA at `licenses/NVIDIA-CUDA-EULA-13.4.1.html`. The archive root also contains `provenance.json`.

License: NVIDIA CUDA Toolkit End User License Agreement, Release 13.4.1
for linux x86_64 and linux arm64, including the CUDA Toolkit
Supplement, Attachment A, and Attachment B.

These files are proprietary NVIDIA software. They are not licensed under
solstone's AGPL-3.0-only license or the llama.cpp MIT license. Their use and
redistribution remain subject to the NVIDIA CUDA Toolkit EULA. A verbatim
copy of the unmodified 13.4.1 EULA accompanies each selected linux archive at
`licenses/NVIDIA-CUDA-EULA-13.4.1.html`. NVIDIA does not sponsor or endorse solstone.

sol pbc redistributes these CUDA components from `updates.solstone.app` only as
Attachment-A distributable portions: unmodified except for unzipping, inside
the solstone application with material additional functionality, and not as a
stand-alone SDK distribution. Their redistribution is permitted only within
those NVIDIA CUDA Toolkit EULA bounds.


### llama.cpp hash components

the llama.cpp b11669 runtimes include the following hash components.
their upstream license texts are reproduced below.

Source: https://github.com/ggml-org/llama.cpp/tree/620d2225cc436ab0b001e3200e590e3b286ef9ab/vendor/hash

#### xxHash

Copyright (C) 2012-2023 Yann Collet

xxHash Library
Copyright (c) 2012-2021 Yann Collet
All rights reserved.

BSD 2-Clause License (https://www.opensource.org/licenses/bsd-license.php)

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

* Redistributions of source code must retain the above copyright notice, this
  list of conditions and the following disclaimer.

* Redistributions in binary form must reproduce the above copyright notice, this
  list of conditions and the following disclaimer in the documentation and/or
  other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

#### rotate-bits

MIT License

Copyright (c) 2021 William Casarin <jb55@jb55.com>

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

#### SHA-256

2010-06-11 : Igor Pavlov : Public domain

#### SHA-1

the upstream source carries this attribution:

```text
SHA-1 in C
By Steve Reid <steve@edmweb.com>
100% Public Domain
```

## runtime-downloaded provider artifacts (llama.cpp Vulkan/CPU)

These artifacts are downloaded on demand into the journal provider cache for
solstone's local inference runtime on supported macos and linux systems. They
are distributed as application components, not as stand-alone runtime
distributions.
sol pbc redistributes them from `updates.solstone.app`; they are not bundled in
this repository.

### llama.cpp Vulkan/CPU runtime

Files: `llama-server`, extracted from
`llama-b11669-bin-macos-arm64.tar.gz`,
`llama-b11669-bin-ubuntu-vulkan-arm64.tar.gz`, and
`llama-b11669-bin-ubuntu-vulkan-x64.tar.gz`.

Source: https://github.com/ggml-org/llama.cpp

License: MIT License.

The complete llama.cpp MIT license and copyright notice is reproduced in
`licenses/llama.cpp-LICENSE.txt` and accompanies each runtime artifact.
The MIT license permits sol pbc's redistribution of these Vulkan/CPU runtime
files.

## bundled backup tools (restic and rclone)

The journal packages include the unmodified upstream release binaries of
restic 0.19.0 (https://github.com/restic/restic/releases/tag/v0.19.0) and
rclone 1.74.4 (https://github.com/rclone/rclone/releases/tag/v1.74.4). On
linux and macos they are at `lib/solstone-restic/restic` and
`lib/solstone-rclone/rclone`; on windows at `lib/solstone-restic/restic.exe`
and `lib/solstone-rclone/rclone.exe`. They are not part of solstone and are
not covered by solstone's AGPL-3.0-only license.

restic is licensed under the BSD 2-Clause License and rclone under the MIT
License. Both are Go programs, and Go links every module they use into the
binary. The complete license texts of
every Go module linked into each binary, with a machine-readable attribution
list, are at `share/solstone-journal/licenses/restic/` and
`share/solstone-journal/licenses/rclone/` (windows: `share/licenses/restic/`
and `share/licenses/rclone/`). The source of each tool and module is published
at the versions those lists record.

rclone includes one library under the GNU Lesser General Public License v3
(LGPL-3.0): `github.com/cloudsoda/sddl`, version
`v0.0.0-20250224235906-926454e91efc`. Its license text and the GNU GPL v3 text
it builds on are in the rclone license directory named above. To rebuild rclone
with a modified copy of that library, use rclone 1.74.4's published source (tag
`v1.74.4`, commit `5bc93a2a7ab0ebd0a11352bc4968eabeffb18027`, at
https://github.com/rclone/rclone) and the library's source at that version
(GitHub commit `926454e` of https://github.com/CloudSoda/sddl, or
proxy.golang.org). Point rclone's Go module build at your copy, for example
with a `replace` directive in `go.mod`, and rebuild.

## Microsoft Visual C++ runtime (windows)

The windows package includes unmodified Microsoft redistributable runtime
libraries in its application directory. They are not part of solstone and are
not covered by solstone's AGPL-3.0-only license.

Attribution: Microsoft Corporation.

Source:

- Package: Microsoft.VC.14.44.17.14.CRT.Redist.X64.base.vsix
  (SHA-256 4aaf54db0bfc9435f7c3660e1a00237a4b556042bfeea64bde44c2e0194e6ee5)
  from download.visualstudio.microsoft.com
- Toolset: Microsoft.VC143.CRT and Microsoft.VC143.OpenMP, 14.44.35112

| Bundled file | SHA-256 |
|---|---|
| `lib/solstone-native/msvcp140.dll` | `0f885b509a685d2bbfa652fed26b5fb31d88fbdab0a978c641d1c7b8aa460aa9` |
| `lib/solstone-native/vcruntime140.dll` | `d5e4d9a3e835fa679450145d6a7d94e36573a509317111904d9b3712c30d9066` |
| `lib/solstone-native/vcruntime140_1.dll` | `1f2d41c4aa5db0bc33ebf7b66d72943a817d7ce6cbe880502a9403823633093f` |
| `lib/solstone-native/vcomp140.dll` | `55aba23cdcd6484fbb06f4155b8ca75adfce7a881f10afd0c49457165e677164` |
| `lib/solstone-nvattest/msvcp140.dll` | `0f885b509a685d2bbfa652fed26b5fb31d88fbdab0a978c641d1c7b8aa460aa9` |
| `lib/solstone-nvattest/vcruntime140.dll` | `d5e4d9a3e835fa679450145d6a7d94e36573a509317111904d9b3712c30d9066` |
| `lib/solstone-nvattest/vcruntime140_1.dll` | `1f2d41c4aa5db0bc33ebf7b66d72943a817d7ce6cbe880502a9403823633093f` |

License notice: sol pbc redistributes these components under the Distributable
Code terms of the Microsoft Visual Studio 2022 license, published at
https://visualstudio.microsoft.com/license-terms/ (Distributable List:
https://aka.ms/vs/17/redistribution). They are licensed separately by
Microsoft. You may redistribute them only as part of this program, unmodified,
and you may not reverse engineer, decompile or disassemble them, or remove or
alter any Microsoft notice they carry. Microsoft provides these files as is and
gives no warranty for them. Microsoft does not sponsor or endorse solstone.
The same restrictions are restated in the installed package at
share/licenses/msvc/NOTICE.md.

## NVIDIA GPU attestation verifier

The windows package includes the NVIDIA GPU attestation verifier
(`bin/nvattest.exe`) and the CA bundle it uses (`share/ca/ca-bundle.pem`).
They are not part of solstone and are not covered by solstone's AGPL-3.0-only
license.

Attribution: NVIDIA Corporation. sol pbc's changes are marked in the source.

Source:

- Verifier: https://github.com/solpbc/attestation-sdk, revision
  ff957aa1012781d18b68973e370a0188b8ef502d, built from source. The build
  receipt is at share/provenance/nvattest/receipt.json.
- CA bundle: https://curl.se/ca/cacert-2026-07-16.pem
  (SHA-256 3ff344e30b9b1ed2971044eabb438a08f2e2245ddb5f8ab1a3ad8b63ab4eaf91)

The verifier is licensed under the Apache License 2.0, at
share/licenses/nvattest/LICENSE. The CA bundle is derived from Mozilla's root
certificate store and is licensed under the Mozilla Public License 2.0. The
notices for the CA bundle and for the libraries built into the verifier are at
share/licenses/nvattest/NOTICES.md.

The linux and macos packages include the verifier and its library, with the
same CA bundle, at `lib/solstone-nvattest/`. These components are not part of
solstone and are not covered by solstone's AGPL-3.0-only license.

Source:

- Verifier and library: version 1.2.2-sol.6, built by sol pbc from
  https://github.com/solpbc/attestation-sdk revision
  fdc3c39958f12ba1055ae5beebc182577f7430b3, based on NVIDIA's upstream
  revision 73efa3ac1bec28ed7d7f0c0811a6c993e722dbd4. The windows revision
  above is this revision plus its windows build changes.
- CA bundle: https://curl.se/ca/cacert-2026-07-16.pem, the same file as on
  windows (SHA-256
  3ff344e30b9b1ed2971044eabb438a08f2e2245ddb5f8ab1a3ad8b63ab4eaf91).

As on windows, the verifier and library are licensed under the Apache License
2.0 and the CA bundle under the Mozilla Public License 2.0. The license and the
notices shipped with this build, including the CA bundle's, are at
share/solstone-journal/licenses/nvattest/.

## Microsoft Edge WebView2 loader (windows)

The journal app for windows (`bin/journal-app.exe`) embeds the Microsoft Edge
WebView2 loader, linked statically. The loader is not part of solstone and is
not covered by solstone's AGPL-3.0-only license.

Attribution: Microsoft Corporation.

Source:

- Package: Microsoft.Web.WebView2 1.0.3650.58 from nuget.org
  (SHA-256 911a472128c82ac8baa0c486c23342cc9dd6e7dc50d754e676726642ca065c60),
  member build/native/x64/WebView2LoaderStatic.lib
- Carried by: the webview2-com-sys 0.38.2 crate, member
  x64/WebView2LoaderStatic.lib, byte-identical to that package member

| Linked library | SHA-256 |
|---|---|
| `WebView2LoaderStatic.lib` (x64) | `0659b741bde6348d4c4a6ec4ceb9af50e3d0048ed9cd3c8659bccbb61fde55ee` |

License notice: BSD 3-Clause License (BSD-3-Clause), from LICENSE.txt in the
NuGet package. The webview2-com-sys crate's MIT license does not cover this
library. The complete license text is reproduced in the installed package at
share/licenses/webview2-loader/NOTICE.md.

## the journal app for mac

`journal.app` (built from `platforms/macos/`) redistributes the two items below
in addition to the native runtime it embeds, whose notices are above and travel
with each payload. Its other resolved Swift packages are source dependencies of
the shared Mac libraries and are not linked into the app.

## Sparkle

- project: sparkle-project/Sparkle
- version: 2.9.4 with a downstream download-origin restriction, supplied by the exact `solstone-macos` support revision in `platforms/macos/Package.swift`
- patch and build provenance: [solstone-macos/vendor/sparkle](https://github.com/solpbc/solstone-macos/tree/main/vendor/sparkle)
- license: see below
- source: https://github.com/sparkle-project/Sparkle/tree/2.9.4

Copyright (c) 2006-2013 Andy Matuschak.
Copyright (c) 2009-2013 Elgato Systems GmbH.
Copyright (c) 2011-2014 Kornel Lesiński.
Copyright (c) 2015-2017 Mayur Pawashe.
Copyright (c) 2014 C.W. Betts.
Copyright (c) 2014 Petroules Corporation.
Copyright (c) 2014 Big Nerd Ranch.
All rights reserved.

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

=================
EXTERNAL LICENSES
=================

bspatch.c and bsdiff.c, from bsdiff 4.3 <http://www.daemonology.net/bsdiff/>:

Copyright 2003-2005 Colin Percival
All rights reserved

Redistribution and use in source and binary forms, with or without
modification, are permitted providing that the following conditions 
are met:
1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in the
   documentation and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE AUTHOR ``AS IS'' AND ANY EXPRESS OR
IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED.  IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY
DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT,
STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING
IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.

--

sais.c and sais.h, from sais-lite (2010/08/07) <https://sites.google.com/site/yuta256/sais>:

The sais-lite copyright is as follows:

Copyright (c) 2008-2010 Yuta Mori All Rights Reserved.

Permission is hereby granted, free of charge, to any person
obtaining a copy of this software and associated documentation
files (the "Software"), to deal in the Software without
restriction, including without limitation the rights to use,
copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice shall be
included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES
OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT
HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
OTHER DEALINGS IN THE SOFTWARE.

--

Portable C implementation of Ed25519, from https://github.com/orlp/ed25519

Copyright (c) 2015 Orson Peters <orsonpeters@gmail.com>

This software is provided 'as-is', without any express or implied warranty. In no event will the
authors be held liable for any damages arising from the use of this software.

Permission is granted to anyone to use this software for any purpose, including commercial
applications, and to alter it and redistribute it freely, subject to the following restrictions:

1. The origin of this software must not be misrepresented; you must not claim that you wrote the
   original software. If you use this software in a product, an acknowledgment in the product
   documentation would be appreciated but is not required.

2. Altered source versions must be plainly marked as such, and must not be misrepresented as
   being the original software.

3. This notice may not be removed or altered from any source distribution.

--

SUSignatureVerifier.m:

Copyright (c) 2011 Mark Hamlin.

All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted providing that the following conditions
are met:
1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in the
   documentation and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE AUTHOR ``AS IS'' AND ANY EXPRESS OR
IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED.  IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY
DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT,
STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING
IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.

## Comfortaa

- project: Comfortaa (https://github.com/alexeiva/comfortaa)
- file: `Comfortaa-Bold.ttf` in the journal mark resource bundle, from solstone-macos's `Sources/JournalMarkKit/Resources/` at the pinned commit
- license: SIL Open Font License 1.1, shipped beside the font as `Comfortaa-OFL.txt`

```
Copyright 2011 The Comfortaa Project Authors (https://github.com/alexeiva/comfortaa), with Reserved Font Name "Comfortaa".

This Font Software is licensed under the SIL Open Font License, Version 1.1.
This license is copied below, and is also available with a FAQ at:
http://scripts.sil.org/OFL


-----------------------------------------------------------
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007
-----------------------------------------------------------

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font creation
efforts of academic and linguistic communities, and to provide a free and
open framework in which fonts may be shared and improved in partnership
with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded, 
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply
to any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software components as
distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting -- in part or in whole -- any of the components of the
Original Version, by changing formats or by porting the Font Software to a
new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed, modify,
redistribute, and sell modified and unmodified copies of the Font
Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components,
in Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the corresponding
Copyright Holder. This restriction only applies to the primary font name as
presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created
using the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.
```
