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
Any `response_format` is rejected with HTTP 400. Completion length is capped at
the context space remaining after the prepared prompt; an oversized prompt is
rejected. A model-loading failure exits the process.

From this directory, run the weight-free contract tests with:

```sh
xcodebuild -scheme mlx-helper -destination 'platform=macOS,arch=arm64' \
  -configuration Release -derivedDataPath DerivedData \
  -skipPackagePluginValidation -skipMacroValidation test
```

They exercise the HTTP routes in memory and do not initialize the GPU.
Use `make mlx-helper-clean` to remove this package's build outputs.
