# ffbuildtool

Library to validate & create manifests for FusionFall builds/versions.

Current features:
- Generate a full version manifest from a path containing compressed asset bundles
- Validate compressed asset bundles given a manifest
- Validate uncompressed asset bundles given a manifest

## Building

```
cargo build
```

By default, the crate requires liblzma to be installed on the system or it won't build. You can get around this with `--no-default-features` but this will cause uncompressed files to be missing from any created manifests.

## Running Unit Tests
```
cargo test
```

## Examples

See `examples`