$ErrorActionPreference = 'Stop'
$source = @'
using System;
using System.ComponentModel;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Security.AccessControl;
using System.Security.Principal;
using Microsoft.Win32.SafeHandles;

public static class FixtureAclCheck {
    [StructLayout(LayoutKind.Sequential)]
    private struct SidAndAttributes {
        public IntPtr Sid;
        public uint Attributes;
    }

    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool CreateRestrictedToken(
        IntPtr existing, uint flags, uint disableCount, IntPtr disabled,
        uint privilegeCount, IntPtr privileges, uint restrictCount,
        [In] SidAndAttributes[] restricted, out SafeAccessTokenHandle token);

    public static void Run(string root, bool privateAcl) {
        Directory.CreateDirectory(root);
        try {
            using (var identity = WindowsIdentity.GetCurrent()) {
                var capability = new SecurityIdentifier("S-1-5-21-10-20-30-40");
                var everyone = new SecurityIdentifier(WellKnownSidType.WorldSid, null);
                var logon = identity.Groups.Cast<SecurityIdentifier>()
                    .Single(sid => sid.Value.StartsWith("S-1-5-5-"));
                var inheritance = InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit;
                if (privateAcl) {
                    var acl = new DirectorySecurity();
                    acl.SetAccessRuleProtection(true, false);
                    acl.AddAccessRule(new FileSystemAccessRule(identity.User,
                        FileSystemRights.FullControl, inheritance,
                        PropagationFlags.None, AccessControlType.Allow));
                    acl.AddAccessRule(new FileSystemAccessRule(everyone,
                        FileSystemRights.ReadAndExecute, inheritance,
                        PropagationFlags.None, AccessControlType.Allow));
                    new DirectoryInfo(root).SetAccessControl(acl);
                }
                Console.WriteLine("privateAcl=" + privateAcl);
                Console.WriteLine("root=" + root);
                Console.WriteLine("root SDDL=" + new DirectoryInfo(root).GetAccessControl().GetSecurityDescriptorSddlForm(AccessControlSections.All));
                var workspace = Path.Combine(root, "workspace");
                var outside = Path.Combine(root, "outside");
                Directory.CreateDirectory(workspace);
                Directory.CreateDirectory(outside);
                var writableAcl = new DirectoryInfo(workspace).GetAccessControl();
                writableAcl.AddAccessRule(new FileSystemAccessRule(capability,
                    FileSystemRights.ReadAndExecute | FileSystemRights.Write | FileSystemRights.Delete,
                    inheritance, PropagationFlags.None, AccessControlType.Allow));
                new DirectoryInfo(workspace).SetAccessControl(writableAcl);
                var protectedDir = Path.Combine(workspace, ".git");
                Directory.CreateDirectory(protectedDir);
                var deniedAcl = new DirectoryInfo(protectedDir).GetAccessControl();
                deniedAcl.AddAccessRule(new FileSystemAccessRule(capability,
                    FileSystemRights.Write | FileSystemRights.Delete,
                    inheritance, PropagationFlags.None, AccessControlType.Deny));
                new DirectoryInfo(protectedDir).SetAccessControl(deniedAcl);
                var writableFile = Path.Combine(workspace, "delete.txt");
                var outsideFile = Path.Combine(outside, "delete.txt");
                File.WriteAllText(writableFile, "workspace");
                File.WriteAllText(outsideFile, "outside");
                foreach (var path in new [] { workspace, outside, protectedDir }) {
                    Console.WriteLine(path + " SDDL=" + new DirectoryInfo(path).GetAccessControl().GetSecurityDescriptorSddlForm(AccessControlSections.All));
                }
                var sids = new [] { capability, logon, everyone };
                var entries = new SidAndAttributes[sids.Length];
                SafeAccessTokenHandle token;
                try {
                    for (int i = 0; i < sids.Length; i++) {
                        var bytes = new byte[sids[i].BinaryLength];
                        sids[i].GetBinaryForm(bytes, 0);
                        entries[i].Sid = Marshal.AllocHGlobal(bytes.Length);
                        Marshal.Copy(bytes, 0, entries[i].Sid, bytes.Length);
                    }
                    // Match the legacy backend: disable privileges, LUA token, write restrictions.
                    if (!CreateRestrictedToken(identity.AccessToken.DangerousGetHandle(), 0xD,
                        0, IntPtr.Zero, 0, IntPtr.Zero, (uint)entries.Length, entries, out token)) {
                        throw new Win32Exception(Marshal.GetLastWin32Error());
                    }
                } finally {
                    foreach (var entry in entries) {
                        if (entry.Sid != IntPtr.Zero) Marshal.FreeHGlobal(entry.Sid);
                    }
                }
                using (token) {
                    WindowsIdentity.RunImpersonated(token, () => {
                        foreach (var path in new [] { writableFile, outsideFile }) {
                            try { File.Delete(path); Console.WriteLine("deleted " + path); }
                            catch (Exception error) { Console.WriteLine("denied " + path + ": " + error.Message); }
                        }
                        try { Directory.Delete(protectedDir); Console.WriteLine("deleted " + protectedDir); }
                        catch (Exception error) { Console.WriteLine("denied " + protectedDir + ": " + error.Message); }
                    });
                }
                Console.WriteLine("RESULT writable=" + File.Exists(writableFile)
                    + " outside=" + File.Exists(outsideFile) + " protected=" + Directory.Exists(protectedDir));
            }
        } finally {
            Directory.Delete(root, true);
        }
    }
}
'@
Add-Type -TypeDefinition $source
$root = Join-Path $env:GITHUB_WORKSPACE ('codex-rs/.tmp-acl-' + [Guid]::NewGuid().ToString('N'))
[FixtureAclCheck]::Run($root + '-inherited', $false)
[FixtureAclCheck]::Run($root + '-private', $true)
