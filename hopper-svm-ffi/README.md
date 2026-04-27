# hopper-svm-ffi

C-ABI bindings for `hopper-svm`.

This crate is an optional workspace member in the standalone
[BluefootLabs/hopper-svm](https://github.com/BluefootLabs/hopper-svm) repo. It
exposes a stable `extern "C"` surface for future TypeScript and Python bindings.

Build directly when needed:

```sh
cargo build --release -p hopper-svm-ffi
cargo build --release -p hopper-svm-ffi --features bpf-execution
```

License: MIT OR Apache-2.0.
