# AWS configuration dependency

Source: published `aws-config` 1.8.12 from the AWS Rust SDK.
Archive: https://crates.io/api/v1/crates/aws-config/1.8.12/download
SHA-256: `96571e6996817bf3d58f6b569e4b9fd2e9d2fcf9f7424eed07b2ce9bb87535e5`

The published crate uses P-256 0.13, which retains the affected crypto-bigint
0.5 series. This copy upgrades P-256 to 0.14 and rand to 0.10. The AWS login
DPoP adapter uses the corresponding SEC1 and operating-system RNG APIs.
A regression test verifies the generated signature and rejects a modified
payload. AWS login remains enabled.

Cargo and Bazel both consume this directory through the workspace path patch.
The crate version is unchanged. Remove this copy when a published AWS release
uses the fixed crypto dependency. The upstream license and source are retained;
the crate-local Cargo.lock is omitted because the workspace lockfile is used.
