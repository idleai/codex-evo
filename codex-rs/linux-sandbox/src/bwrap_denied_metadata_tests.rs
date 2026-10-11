use super::*;
use codex_protocol::protocol::FileSystemAccessMode;
use codex_protocol::protocol::FileSystemPath;
use codex_protocol::protocol::FileSystemSandboxEntry;
use codex_protocol::protocol::FileSystemSpecialPath;
use tempfile::TempDir;

#[test]
fn missing_denied_metadata_has_one_compatible_mount_target() {
    let root = TempDir::new().expect("workspace");
    for name in [".git", ".agents", ".codex"] {
        let denied = root.path().join(name);
        let policy = FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry::new(
                FileSystemPath::Special {
                    value: FileSystemSpecialPath::Root,
                },
                FileSystemAccessMode::Read,
            ),
            FileSystemSandboxEntry::new(
                FileSystemPath::Special {
                    value: FileSystemSpecialPath::project_roots(/*subpath*/ None),
                },
                FileSystemAccessMode::Write,
            ),
            FileSystemSandboxEntry::new(
                FileSystemPath::Path {
                    path: AbsolutePathBuf::from_absolute_path(&denied)
                        .expect("denied path")
                        .into(),
                },
                FileSystemAccessMode::Deny,
            ),
        ]);
        let args = create_filesystem_args(&policy, root.path(), BwrapOptions::default())
            .expect("sandbox arguments");
        let path = path_to_string(&denied);
        assert!(
            !args
                .args
                .windows(3)
                .any(|args| args[0] == "--ro-bind-data" && args[2] == path)
        );
        assert!(
            args.args
                .windows(4)
                .any(|args| args == ["--perms", "000", "--tmpfs", &path])
        );
        assert!(
            !denied.exists(),
            "constructing masks must not create repository paths"
        );
    }
}
