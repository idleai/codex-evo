# RTC crypto compatibility patch

Source: published `rtc-shared` 0.20.5 from the webrtc-rs project.
Archive: https://crates.io/api/v1/crates/rtc-shared/0.20.5/download
SHA-256: `393280ae9959b5245df9eb38a5cd02b373a98021599d36111c339a4696bf8488`

This copy retains the RTC 0.20 interfaces used by the voice host while
upgrading P-256 to 0.14 and SEC1 to 0.8, removing crypto-bigint 0.5.
Only the dependency declarations are changed; the library source is unchanged.

Cargo and Bazel consume this directory through the workspace path patch.
Remove this copy when a compatible published release uses the fixed crypto
dependencies. The upstream license and source are retained; the crate-local
Cargo.lock is omitted because the workspace lockfile is used.

The license files are copied unchanged from upstream commit
`69ebe02fb1ba8d4fd51cce76c579c68fafb6b04b`, matching `.cargo_vcs_info.json`.
