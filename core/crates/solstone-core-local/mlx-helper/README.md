# mlx-helper

A standalone Swift MLX inference server for Apple Silicon on macOS 15 or later.
The journal does not use it yet, and release packages do not include it.

From the repository root, build with `make mlx-helper`. This uses Xcode and its
Metal toolchain and prints the executable and `mlx-swift_Cmlx.bundle` paths.
Keep that resource bundle beside the executable. `swift build` does not compile
the Metal shaders needed at runtime.

Run the executable with `--model /absolute/path/to/local/model`; `--help` lists
the options. All model and tokenizer files must already be in that directory.
The server accepts literal loopback addresses only and serializes inference
requests in FIFO order. It supports `/health`, `/props`, `/tokenize`, and
non-streaming `/v1/chat/completions` with text and base64 image data URLs.
Structured output supports `response_format` with `json_object` or
`json_schema` and `strict: true`. Schema text is compiled in its original field
order. Compiled grammars are cached by exact schema text, with a new matcher
for every request. The allocator cache defaults to 512 MiB; use
`--cache-limit-mb` to set another positive limit. Completion length is capped at
the context space remaining after the prepared prompt; an oversized prompt is
rejected. Budget exhaustion returns `finish_reason: "length"` with the partial
text and completion usage, including grammar-forced tokens. A model-loading
failure exits the process.

From this directory, run the weight-free contract tests with:

```sh
xcodebuild -scheme mlx-helper -destination 'platform=macOS,arch=arm64' \
  -configuration Release -derivedDataPath DerivedData \
  -skipPackagePluginValidation -skipMacroValidation ENABLE_TESTABILITY=YES test
```

The tests exercise HTTP routes in memory and the guided loop on synthetic,
weight-free models. Run them through Xcode so MLX's Metal resources are built.
Use the executable from `make mlx-helper` for performance measurements; the test
action can instrument its outputs for coverage.
Use `make mlx-helper-clean` to remove this package's build outputs.

The `Sources/GuidedGeneration` target carries seven Swift support files from
`MLXGuidedGeneration` at the revision pinned in `Package.swift`, under their
[MIT license](Sources/GuidedGeneration/LICENSE). The loop feeds each sampled
token together with the following forced tokens, honors the caller's sampler
and processor, and expands packed grammar masks on the GPU. The bridge creates
fresh matchers over compiled grammars. It uses the upstream package's C bindings
and ordinary unquantized model caches; its package-private KV planning code is
not copied. The remaining model and tokenizer code stays upstream.
