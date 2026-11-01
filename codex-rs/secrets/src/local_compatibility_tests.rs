use super::*;
use pretty_assertions::assert_eq;

#[test]
fn decrypts_data_written_by_published_age() -> Result<()> {
    // Generated with the published age 0.12.1 and the previous Cargo.lock.
    let ciphertext = BASE64_STANDARD.decode(
        "YWdlLWVuY3J5cHRpb24ub3JnL3YxCi0+IHNjcnlwdCAxdUdDUUpMNlNjMXhSZHhrY1VvTDV3IDEzCktPTThUcHJoZVdBMUl5QVJHV2FxVzlCVWMySTdQZTJzcVZUTXkzWGd4NTgKLS0tIFp0T1U1dVRIaEs2U0lDNUVwS0hoUVZYMjBNSVYxeW1PMit6TkxNNTV4NG8KPsMiQoQx3T9gyHmvOSxl74U+muk4WqvuIYoxwt/xAa+UL6n+lIvCSHD64LbXrfwZ7p13kkFvSa7P9e8hoHsX7m1S4w==",
    )?;
    let passphrase = SecretString::from("codex-age-compatibility".to_owned());
    assert_eq!(
        decrypt_with_passphrase(&ciphertext, &passphrase)?,
        b"codex-secrets compatibility fixture",
    );
    Ok(())
}
