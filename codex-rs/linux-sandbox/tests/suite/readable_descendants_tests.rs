//! Narrow read grants remain usable beneath denied directories.

use super::LONG_TIMEOUT_MS;
use super::codex_linux_sandbox_exe;
use super::create_env_from_core_vars;
use super::run_cmd_result_with_permission_profile_for_cwd;
use super::should_skip_bwrap_tests;
use codex_protocol::models::PermissionProfile;
use codex_protocol::permissions::FileSystemAccessMode;
use codex_protocol::permissions::FileSystemPath;
use codex_protocol::permissions::FileSystemSandboxEntry;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::permissions::FileSystemSpecialPath;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;

enum ChildGrants {
    ReadOnly,
    NestedWritable,
}

#[test_case::test_case(FileSystemAccessMode::Read, ChildGrants::ReadOnly; "read_parent_read_child")]
#[test_case::test_case(FileSystemAccessMode::Write, ChildGrants::ReadOnly; "write_parent_read_child")]
#[test_case::test_case(FileSystemAccessMode::Read, ChildGrants::NestedWritable; "read_parent_write_grandchild")]
#[test_case::test_case(FileSystemAccessMode::Write, ChildGrants::NestedWritable; "write_parent_write_grandchild")]
#[tokio::test]
async fn sandbox_reopens_read_grants_under_denied_parents(
    parent_access: FileSystemAccessMode,
    child_grants: ChildGrants,
) {
    if should_skip_bwrap_tests().await {
        eprintln!("skipping bwrap test: bwrap sandbox prerequisites are unavailable");
        return;
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = AbsolutePathBuf::try_from(temp.path()).expect("absolute workspace");
    let blocked = cwd.join("blocked");
    let readable = blocked.join("readable");
    let writable = readable.join("writable");
    let nested_denied = readable.join("hidden");
    std::fs::create_dir_all(&writable).expect("create writable descendant");
    std::fs::create_dir_all(&nested_denied).expect("create nested denied directory");
    let files = [
        (blocked.join("secret.txt"), "private sibling\n"),
        (blocked.join("allowed.txt"), "allowed file\n"),
        (readable.join("note.txt"), "allowed directory\n"),
        (nested_denied.join("secret.txt"), "private descendant\n"),
    ];
    for (path, contents) in &files {
        std::fs::write(path, contents).expect("seed fixture");
    }

    let sandbox_helper = codex_linux_sandbox_exe();
    let helper_dir = AbsolutePathBuf::try_from(sandbox_helper.parent().expect("helper parent"))
        .expect("absolute helper directory");
    let mut entries = vec![
        FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::Minimal,
            },
            FileSystemAccessMode::Read,
        ),
        FileSystemSandboxEntry::new(helper_dir.into(), FileSystemAccessMode::Read),
        FileSystemSandboxEntry::new(cwd.clone().into(), parent_access),
    ];
    for (path, access) in [
        (blocked.as_path(), FileSystemAccessMode::Deny),
        (files[1].0.as_path(), FileSystemAccessMode::Read),
        (readable.as_path(), FileSystemAccessMode::Read),
        (nested_denied.as_path(), FileSystemAccessMode::Deny),
    ] {
        entries.push(FileSystemSandboxEntry::new(
            AbsolutePathBuf::try_from(path)
                .expect("absolute grant")
                .into(),
            access,
        ));
    }
    if matches!(child_grants, ChildGrants::NestedWritable) {
        entries.push(FileSystemSandboxEntry::new(
            writable.clone().into(),
            FileSystemAccessMode::Write,
        ));
    }
    let script = r#"set -eu
/bin/cat blocked/allowed.txt blocked/readable/note.txt
if /bin/ls blocked >/dev/null 2>&1; then exit 10; fi
for hidden in blocked/secret.txt blocked/readable/hidden/secret.txt; do
    if (: < "$hidden") 2>/dev/null; then exit 11; fi
done
for protected in blocked/allowed.txt blocked/readable/note.txt; do
    if (printf changed > "$protected") 2>/dev/null; then exit 12; fi
done
if [ "$1" = writable ]; then
    printf 'written\n' > blocked/readable/writable/new.txt
    /bin/cat blocked/readable/writable/new.txt
elif (printf changed > blocked/readable/writable/new.txt) 2>/dev/null; then
    exit 13
fi
"#;
    let (mode, expected_stdout) = match child_grants {
        ChildGrants::ReadOnly => ("readonly", "allowed file\nallowed directory\n"),
        ChildGrants::NestedWritable => ("writable", "allowed file\nallowed directory\nwritten\n"),
    };
    let output = run_cmd_result_with_permission_profile_for_cwd(
        &["/bin/sh", "-c", script, "read-grants", mode],
        cwd,
        PermissionProfile::from_runtime_permissions(
            &FileSystemSandboxPolicy::restricted(entries),
            NetworkSandboxPolicy::Enabled,
        ),
        create_env_from_core_vars(),
        LONG_TIMEOUT_MS,
        /*use_legacy_landlock*/ false,
    )
    .await
    .expect("sandbox should preserve narrow read grants");

    assert_eq!(
        (output.exit_code, output.stdout.text, output.stderr.text),
        (0, expected_stdout.to_string(), String::new())
    );
    for (path, contents) in files {
        assert_eq!(
            std::fs::read_to_string(path).expect("host fixture"),
            contents
        );
    }
}
