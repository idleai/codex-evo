//! Deny-read mounts with explicit grants to narrower paths.

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use codex_protocol::error::CodexErr;
use codex_protocol::error::Result;

use super::BwrapArgs;
use super::SyntheticMountTargetKind;
use super::append_daemon_socket_masks;
use super::append_empty_file_bind_data_args;
use super::append_missing_empty_file_bind_data_args;
use super::append_mount_target_parent_dir_args;
use super::find_first_non_existent_component;
use super::first_writable_symlink_component_in_path;
use super::is_within_allowed_write_paths;
use super::path_depth;
use super::path_to_string;

pub(super) fn append_unreadable_root_args(
    bwrap_args: &mut BwrapArgs,
    unreadable_root: &Path,
    allowed_write_paths: &[PathBuf],
    allowed_read_only_paths: &[PathBuf],
    daemon_directories: &BTreeSet<PathBuf>,
) -> Result<()> {
    if let Some(symlink) =
        first_writable_symlink_component_in_path(unreadable_root, allowed_write_paths)
    {
        /*
         * Deny-read masks must fail closed when the protected path crosses a
         * symlink that remains writable to the sandboxed process. Resolving and
         * masking the symlink's current target is a TOCTTOU snapshot: bwrap would
         * protect the old target while the logical path could later point
         * somewhere else.
         */
        return Err(CodexErr::Fatal(format!(
            "cannot enforce sandbox deny-read path {} because it crosses writable symlink {}",
            unreadable_root.display(),
            symlink.display()
        )));
    }

    if !unreadable_root.exists() {
        if let Some(first_missing_component) = find_first_non_existent_component(unreadable_root)
            && is_within_allowed_write_paths(&first_missing_component, allowed_write_paths)
        {
            if bwrap_args.synthetic_mount_targets.iter().any(|target| {
                target.path == first_missing_component
                    && target.kind == SyntheticMountTargetKind::EmptyDirectory
            }) {
                // Metadata protection has already planned a directory here.
                // Keep that mount type when adding the stricter read denial.
                bwrap_args.args.extend([
                    "--perms".to_string(),
                    "000".to_string(),
                    "--tmpfs".to_string(),
                    path_to_string(&first_missing_component),
                    "--remount-ro".to_string(),
                    path_to_string(&first_missing_component),
                ]);
            } else {
                append_missing_empty_file_bind_data_args(bwrap_args, &first_missing_component)?;
            }
        }
        return Ok(());
    }

    append_existing_unreadable_path_args(
        bwrap_args,
        unreadable_root,
        allowed_write_paths,
        allowed_read_only_paths,
        daemon_directories,
    )
}

fn append_existing_unreadable_path_args(
    bwrap_args: &mut BwrapArgs,
    unreadable_root: &Path,
    allowed_write_paths: &[PathBuf],
    allowed_read_only_paths: &[PathBuf],
    daemon_directories: &BTreeSet<PathBuf>,
) -> Result<()> {
    if unreadable_root.is_dir() {
        let mut readable_descendants: Vec<&Path> = allowed_read_only_paths
            .iter()
            .map(PathBuf::as_path)
            .filter(|path| {
                *path != unreadable_root && path.starts_with(unreadable_root) && path.exists()
            })
            .collect();
        let mut allowed_descendants: Vec<&Path> = allowed_write_paths
            .iter()
            .map(PathBuf::as_path)
            .filter(|path| *path != unreadable_root && path.starts_with(unreadable_root))
            .collect();
        allowed_descendants.extend(readable_descendants.iter().copied());
        bwrap_args.args.push("--perms".to_string());
        // Execute-only permissions allow access to explicit child grants while
        // hiding all other contents of the denied directory.
        bwrap_args.args.push(if allowed_descendants.is_empty() {
            "000".to_string()
        } else {
            "111".to_string()
        });
        bwrap_args.args.push("--tmpfs".to_string());
        bwrap_args.args.push(path_to_string(unreadable_root));
        // Create child mount targets before freezing the denied parent. Read
        // grants must be restored here because the empty tmpfs hid their initial
        // binds. Deeper deny masks and writable binds are applied by the caller.
        allowed_descendants.sort_by_key(|path| path_depth(path));
        allowed_descendants.dedup();
        for descendant in allowed_descendants {
            append_mount_target_parent_dir_args(&mut bwrap_args.args, descendant, unreadable_root);
        }
        readable_descendants.sort_by_key(|path| path_depth(path));
        for descendant in readable_descendants {
            bwrap_args.args.extend([
                "--ro-bind".to_string(),
                path_to_string(descendant),
                path_to_string(descendant),
            ]);
            append_daemon_socket_masks(&mut bwrap_args.args, descendant, daemon_directories)?;
        }
        bwrap_args.args.push("--remount-ro".to_string());
        bwrap_args.args.push(path_to_string(unreadable_root));
        return Ok(());
    }

    bwrap_args.args.push("--perms".to_string());
    bwrap_args.args.push("000".to_string());
    append_empty_file_bind_data_args(bwrap_args, unreadable_root)
}
