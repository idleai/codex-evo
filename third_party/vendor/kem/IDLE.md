# Legacy KEM trait API

Source: published `kem` 0.3.0-pre.0 from RustCrypto.
Archive: https://crates.io/api/v1/crates/kem/0.3.0-pre.0/download
SHA-256: `2b8645470337db67b01a7f966decf7d0bafedbae74147d33e641c67a91df239f`

This copy is unchanged. The adjacent ML-KEM 0.2.3 dependency uses it by path
so Clatter's original API can coexist with the stable `kem` 0.3 dependency
used by age. The upstream source and license files are retained; the
crate-local Cargo.lock is omitted because the workspace lockfile is used.

Remove this directory with `../ml-kem` when Clatter supports the stable API.
