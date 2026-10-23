extern crate self as libc;
pub use std::ffi::{c_int, c_uint, c_void};
pub const ENOENT: i32 = 2;
pub const EINVAL: i32 = 22;
pub const ENOTSUP: i32 = 45;
mod access_metadata;
use std::{fs, io, path::Path, process::Command};
fn main() -> io::Result<()> {
    let root = std::env::temp_dir().join(format!("codex-acl-probe-{}", std::process::id()));
    fs::create_dir(&root)?;
    let source_path = root.join("source.jsonl");
    let destination_path = root.join("replacement.tmp");
    fs::write(&source_path, "protected transcript")?;
    let acl = Command::new("/bin/chmod").args(["+a", "everyone deny delete,writesecurity"]).arg(&source_path).output()?;
    assert!(acl.status.success(), "{acl:?}");
    let result = (|| -> io::Result<()> {
        show(&source_path)?;
        let source = fs::File::open(&source_path)?;
        let destination = fs::File::create(&destination_path)?;
        let copied = access_metadata::copy(&source, &destination);
        println!("copy result: {copied:?}");
        show(&destination_path)?;
        assert_eq!(copied.as_ref().err().map(io::Error::kind), Some(io::ErrorKind::PermissionDenied));
        fs::remove_file(&destination_path)?;
        println!("unprotected replacement cleanup passed");
        let deleted = fs::remove_file(&source_path);
        println!("protected source deletion: {deleted:?}");
        assert_eq!(deleted.as_ref().err().map(io::Error::kind), Some(io::ErrorKind::PermissionDenied));
        Ok(())
    })();
    for entry in fs::read_dir(&root)? {
        let output = Command::new("/bin/chmod").arg("-N").arg(entry?.path()).output()?;
        assert!(output.status.success(), "{output:?}");
    }
    fs::remove_dir_all(root)?;
    result
}
fn show(path: &Path) -> io::Result<()> {
    let output = Command::new("/bin/ls").arg("-le").arg(path).output()?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(output.status.success(), "{output:?}");
    Ok(())
}
