# RTC crypto compatibility patch

Source: published `rtc-dtls` 0.20.5 from the webrtc-rs project.
Archive: https://crates.io/api/v1/crates/rtc-dtls/0.20.5/download
SHA-256: `4eb84feb0331735ab54ba58bfeb15300b517c283c5ee45e9f94d8727e6573ec5`

This copy retains the RTC 0.20 interfaces used by the voice host while
upgrading P-256/P-384 to 0.14 and SEC1 to 0.8, removing crypto-bigint 0.5.
The DTLS key generator uses the corresponding SEC1 and operating-system RNG
APIs. A regression test checks key agreement on both NIST curves and rejects invalid
points. Other protocol and transport behavior is unchanged.

Cargo and Bazel consume this directory through the workspace path patch.
Remove this copy when a compatible published release uses the fixed crypto
dependencies. The upstream license and source are retained; the crate-local
Cargo.lock is omitted because the workspace lockfile is used.

The license files are copied unchanged from upstream commit
`69ebe02fb1ba8d4fd51cce76c579c68fafb6b04b`, matching `.cargo_vcs_info.json`.
