$ErrorActionPreference = 'Stop'
$source = @'
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.AccessControl;
using System.Security.Principal;
using System.Text;

public static class AppContainerCheck {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct StartupInfo {
        public uint Size;
        public string Reserved, Desktop, Title;
        public uint X, Y, Width, Height, XChars, YChars, Fill, Flags;
        public ushort Show, ReservedBytes;
        public IntPtr ReservedData, Input, Output, Error;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct StartupInfoEx { public StartupInfo Startup; public IntPtr Attributes; }
    [StructLayout(LayoutKind.Sequential)]
    private struct ProcessInfo { public IntPtr Process, Thread; public uint Pid, Tid; }
    [StructLayout(LayoutKind.Sequential)]
    private struct SecurityCapabilities {
        public IntPtr Package, Capabilities;
        public uint Count, Reserved;
    }
    [DllImport("userenv.dll", CharSet = CharSet.Unicode)]
    private static extern int CreateAppContainerProfile(string name, string display, string description,
        IntPtr capabilities, uint count, out IntPtr sid);
    [DllImport("userenv.dll", CharSet = CharSet.Unicode)]
    private static extern int DeleteAppContainerProfile(string name);
    [DllImport("advapi32.dll")]
    private static extern IntPtr FreeSid(IntPtr sid);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool InitializeProcThreadAttributeList(IntPtr list, int count, uint flags, ref IntPtr size);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool UpdateProcThreadAttribute(IntPtr list, uint flags, IntPtr attribute,
        IntPtr value, IntPtr size, IntPtr previous, IntPtr returned);
    [DllImport("kernel32.dll")]
    private static extern void DeleteProcThreadAttributeList(IntPtr list);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool CreateProcess(string application, StringBuilder command,
        IntPtr processAttributes, IntPtr threadAttributes, bool inherit, uint flags,
        IntPtr environment, string cwd, ref StartupInfoEx startup, out ProcessInfo process);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern uint WaitForSingleObject(IntPtr handle, uint timeout);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetExitCodeProcess(IntPtr process, out uint exitCode);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool TerminateProcess(IntPtr process, uint code);
    [DllImport("kernel32.dll")]
    private static extern bool CloseHandle(IntPtr handle);

    private static uint Start(IntPtr sid, string application, string command, string cwd) {
        IntPtr size = IntPtr.Zero;
        InitializeProcThreadAttributeList(IntPtr.Zero, 1, 0, ref size);
        var attributes = Marshal.AllocHGlobal(size);
        var capabilitySize = Marshal.SizeOf<SecurityCapabilities>();
        var capabilities = Marshal.AllocHGlobal(capabilitySize);
        bool initialized = false;
        try {
            if (!InitializeProcThreadAttributeList(attributes, 1, 0, ref size))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "initialize attributes");
            initialized = true;
            Marshal.StructureToPtr(new SecurityCapabilities { Package = sid }, capabilities, false);
            if (!UpdateProcThreadAttribute(attributes, 0, new IntPtr(0x20009), capabilities,
                new IntPtr(capabilitySize), IntPtr.Zero, IntPtr.Zero))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "set AppContainer identity");
            var startup = new StartupInfoEx { Attributes = attributes };
            startup.Startup.Size = (uint)Marshal.SizeOf<StartupInfoEx>();
            ProcessInfo process;
            if (!CreateProcess(application, new StringBuilder(command), IntPtr.Zero, IntPtr.Zero,
                false, 0x08080000, IntPtr.Zero, cwd, ref startup, out process))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "launch " + application);
            try {
                var wait = WaitForSingleObject(process.Process, 45000);
                if (wait != 0) {
                    TerminateProcess(process.Process, 99);
                    throw new Exception("AppContainer child timed out: " + wait);
                }
                uint exitCode;
                if (!GetExitCodeProcess(process.Process, out exitCode))
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "child exit code");
                return exitCode;
            } finally { CloseHandle(process.Thread); CloseHandle(process.Process); }
        } finally {
            if (initialized) DeleteProcThreadAttributeList(attributes);
            Marshal.FreeHGlobal(attributes);
            Marshal.FreeHGlobal(capabilities);
        }
    }

    public static void Run(string root, string shell, string shellName) {
        string profile = "CodexDeletionProbe" + Guid.NewGuid().ToString("N");
        IntPtr sid = IntPtr.Zero;
        Directory.CreateDirectory(root);
        try {
            int result = CreateAppContainerProfile(profile, profile, "Temporary sandbox check", IntPtr.Zero, 0, out sid);
            if (result < 0) Marshal.ThrowExceptionForHR(result);
            var package = new SecurityIdentifier(sid);
            var inheritance = InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit;
            var acl = new DirectorySecurity();
            acl.SetAccessRuleProtection(true, false);
            acl.AddAccessRule(new FileSystemAccessRule(WindowsIdentity.GetCurrent().User,
                FileSystemRights.FullControl, inheritance, PropagationFlags.None, AccessControlType.Allow));
            acl.AddAccessRule(new FileSystemAccessRule(new SecurityIdentifier(WellKnownSidType.WorldSid, null),
                FileSystemRights.FullControl, inheritance, PropagationFlags.None, AccessControlType.Allow));
            acl.AddAccessRule(new FileSystemAccessRule(package, FileSystemRights.ReadAndExecute,
                inheritance, PropagationFlags.None, AccessControlType.Allow));
            new DirectoryInfo(root).SetAccessControl(acl);
            string workspace = Path.Combine(root, "workspace"), outside = Path.Combine(root, "outside");
            Directory.CreateDirectory(workspace);
            Directory.CreateDirectory(outside);
            var writeAcl = new DirectoryInfo(workspace).GetAccessControl();
            writeAcl.AddAccessRule(new FileSystemAccessRule(package, FileSystemRights.Modify,
                inheritance, PropagationFlags.None, AccessControlType.Allow));
            new DirectoryInfo(workspace).SetAccessControl(writeAcl);
            var protectedDir = Path.Combine(workspace, ".git");
            Directory.CreateDirectory(protectedDir);
            var deniedAcl = new DirectoryInfo(protectedDir).GetAccessControl();
            deniedAcl.AddAccessRule(new FileSystemAccessRule(package, FileSystemRights.Write | FileSystemRights.Delete,
                inheritance, PropagationFlags.None, AccessControlType.Deny));
            new DirectoryInfo(protectedDir).SetAccessControl(deniedAcl);
            string writableFile = Path.Combine(workspace, "delete.txt"), outsideFile = Path.Combine(outside, "delete.txt");
            File.WriteAllText(writableFile, "workspace");
            File.WriteAllText(outsideFile, "outside");
            File.WriteAllText(Path.Combine(root, "read.txt"), "private read succeeded");
            string script = Path.Combine(root, shellName == "cmd" ? "probe.cmd" : "probe.ps1");
            if (shellName == "cmd") {
                File.WriteAllText(script, "@echo off\r\ntype ..\\read.txt\r\necho wrote>delete.txt\r\ndel delete.txt\r\ndel ..\\outside\\delete.txt\r\nrmdir .git\r\ncmd /d /c echo child process succeeded\r\necho shell succeeded\r\n");
            } else {
                File.WriteAllText(script, "$ErrorActionPreference='Continue'\nGet-Content ../read.txt\n[IO.File]::WriteAllText((Join-Path (Get-Location) 'delete.txt'), 'wrote')\n[IO.File]::Delete((Join-Path (Get-Location) 'delete.txt'))\ntry { [IO.File]::Delete((Join-Path (Get-Location) '../outside/delete.txt')) } catch { Write-Output $_.Exception.Message }\ntry { [IO.Directory]::Delete((Join-Path (Get-Location) '.git')) } catch { Write-Output $_.Exception.Message }\n& cmd.exe /d /c 'echo child process succeeded'\nWrite-Output 'shell succeeded'\n");
            }
            string output = Path.Combine(workspace, "output.txt");
            string inner = shellName == "cmd" ? "\"" + script + "\"" : "\"" + shell + "\" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"" + script + "\"";
            string cmd = Path.Combine(Environment.SystemDirectory, "cmd.exe");
            uint exit = Start(sid, cmd, "\"" + cmd + "\" /d /s /c \"" + inner + " >\"" + output + "\" 2>&1\"", workspace);
            string log = File.Exists(output) ? File.ReadAllText(output) : "NO OUTPUT FILE";
            Console.WriteLine("SHELL=" + shellName + " EXIT=" + exit + "\n" + log);
            bool writableDeleted = !File.Exists(writableFile), outsideKept = File.Exists(outsideFile), protectedKept = Directory.Exists(protectedDir);
            Console.WriteLine("RESULT writableDeleted=" + writableDeleted + " outsideKept=" + outsideKept + " protectedKept=" + protectedKept);
            if (exit != 0 || !log.Contains("private read succeeded") || !log.Contains("child process succeeded") || !log.Contains("shell succeeded") || !writableDeleted || !outsideKept || !protectedKept)
                throw new Exception("AppContainer compatibility or deletion boundary check failed for " + shellName);
        } finally {
            if (sid != IntPtr.Zero) { FreeSid(sid); DeleteAppContainerProfile(profile); }
            Directory.Delete(root, true);
        }
    }
}
'@
Add-Type -TypeDefinition $source
$failed = $false
$shells = @{
    cmd = (Join-Path $env:SystemRoot 'System32/cmd.exe')
    powershell = (Join-Path $env:SystemRoot 'System32/WindowsPowerShell/v1.0/powershell.exe')
    pwsh = (Get-Command pwsh).Source
}
foreach ($entry in $shells.GetEnumerator()) {
    $root = Join-Path $env:GITHUB_WORKSPACE ('appcontainer-' + [Guid]::NewGuid().ToString('N'))
    try { [AppContainerCheck]::Run($root, $entry.Value, $entry.Key) }
    catch { $failed = $true; Write-Host $_.Exception.ToString() }
}
if ($failed) { throw 'At least one AppContainer check failed' }
