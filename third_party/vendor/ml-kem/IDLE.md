# Legacy ML-KEM dependency selection

Source: published `ml-kem` 0.2.3 from RustCrypto.
Archive: https://crates.io/api/v1/crates/ml-kem/0.2.3/download
SHA-256: `8de49b3df74c35498c0232031bb7e85f9389f913e2796169c8ab47a53993a18f`

Clatter 2.2 uses this release and the prerelease KEM trait API. The updated
age dependency uses ML-KEM 0.3 and the stable KEM trait API. Cargo cannot
resolve both trait versions from the same registry, and selecting ML-KEM
0.2.1 instead incorrectly binds the old implementation to the stable traits.

Only the `kem` dependency declaration is changed, to use the adjacent copy
of its original published version. The cryptographic implementation and its
tests are unchanged. This preserves the exec-server's existing Noise library.

Cargo and Bazel consume this directory through the workspace path patch.
Remove both this copy and `../kem` when Clatter supports the stable trait API.
The upstream license files and source are retained; the crate-local Cargo.lock
is omitted because the workspace lockfile is used.
