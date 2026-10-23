$ErrorActionPreference = 'Stop'
$source = @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.AccessControl;
using System.Security.Principal;
using System.Text;
using Microsoft.Win32.SafeHandles;

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
    [StructLayout(LayoutKind.Sequential)]
    private struct SidAndAttributes { public IntPtr Sid; public uint Attributes; }
    [StructLayout(LayoutKind.Sequential)]
    private struct TokenGroups { public uint Count; public SidAndAttributes First; }
    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool GetTokenInformation(IntPtr token, int kind,
        IntPtr buffer, uint size, out uint required);
    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool OpenProcessToken(IntPtr process, uint access, out SafeAccessTokenHandle token);
    [DllImport("kernel32.dll")]
    private static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr LoadLibrary(string path);
    [DllImport("kernel32.dll", CharSet = CharSet.Ansi, ExactSpelling = true)]
    private static extern IntPtr GetProcAddress(IntPtr module, string name);
    [DllImport("kernel32.dll")]
    private static extern bool FreeLibrary(IntPtr module);
    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate int QuerySupport(out ulong flags);
    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate int QueryVersion(uint major, out byte available, out uint minor);
    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool CreateRestrictedToken(IntPtr existing, uint flags,
        uint disabledCount, IntPtr disabled, uint privilegeCount, IntPtr privileges,
        uint restrictedCount, [In] SidAndAttributes[] restricted, out SafeAccessTokenHandle token);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool CreateProcessAsUser(SafeAccessTokenHandle token, string application,
        StringBuilder command, IntPtr processAttributes, IntPtr threadAttributes, bool inherit,
        uint flags, IntPtr environment, string cwd, ref StartupInfoEx startup, out ProcessInfo process);
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

    private static SecurityIdentifier LogonSid(IntPtr token) {
        uint needed;
        GetTokenInformation(token, 28, IntPtr.Zero, 0, out needed);
        if (needed == 0) throw new Win32Exception(Marshal.GetLastWin32Error(), "query logon SID size");
        var buffer = Marshal.AllocHGlobal((int)needed);
        try {
            if (!GetTokenInformation(token, 28, buffer, needed, out needed))
                throw new Win32Exception(Marshal.GetLastWin32Error(), "query logon SID");
            var groups = Marshal.PtrToStructure<TokenGroups>(buffer);
            if (groups.Count != 1) throw new Exception("Expected one native logon SID");
            return new SecurityIdentifier(groups.First.Sid);
        } finally { Marshal.FreeHGlobal(buffer); }
    }

    public static void InspectTokenCreation() {
        using (var identity = WindowsIdentity.GetCurrent()) {
            var logon = LogonSid(identity.AccessToken.DangerousGetHandle());
            Console.WriteLine("NATIVE LOGON=" + logon.Value);
            foreach (var systemSid in new [] { "", "S-1-1-0", "S-1-15-2-1", "S-1-15-2-2", "S-1-5-32-545" }) {
                foreach (uint flags in new uint[] { 1, 5, 9, 13 }) {
                    var sids = new List<SecurityIdentifier> {
                        new SecurityIdentifier("S-1-5-21-10-20-30-40"), logon
                    };
                    if (systemSid.Length != 0) sids.Add(new SecurityIdentifier(systemSid));
                    var entries = new SidAndAttributes[sids.Count];
                    try {
                        for (int i = 0; i < sids.Count; i++) {
                            var bytes = new byte[sids[i].BinaryLength];
                            sids[i].GetBinaryForm(bytes, 0);
                            entries[i].Sid = Marshal.AllocHGlobal(bytes.Length);
                            Marshal.Copy(bytes, 0, entries[i].Sid, bytes.Length);
                        }
                        SafeAccessTokenHandle token;
                        bool ok = CreateRestrictedToken(identity.AccessToken.DangerousGetHandle(), flags,
                            0, IntPtr.Zero, 0, IntPtr.Zero, (uint)entries.Length, entries, out token);
                        int error = ok ? 0 : Marshal.GetLastWin32Error();
                        using (token) {
                            Console.WriteLine("TOKEN MATRIX flags=" + flags + " system=" + systemSid + " ok=" + ok + " error=" + error);
                        }
                    } finally {
                        foreach (var entry in entries) Marshal.FreeHGlobal(entry.Sid);
                    }
                }
            }
        }
    }

    private static uint Start(IntPtr sid, string application, string command, string cwd, string backend) {
        IntPtr attributes = IntPtr.Zero, capabilities = IntPtr.Zero;
        bool initialized = false;
        try {
            var startup = new StartupInfoEx();
            startup.Startup.Size = (uint)Marshal.SizeOf<StartupInfoEx>();
            ProcessInfo process;
            if (backend == "appcontainer") {
                IntPtr size = IntPtr.Zero;
                InitializeProcThreadAttributeList(IntPtr.Zero, 1, 0, ref size);
                attributes = Marshal.AllocHGlobal(size);
                if (!InitializeProcThreadAttributeList(attributes, 1, 0, ref size))
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "initialize attributes");
                initialized = true;
                var capabilitySize = Marshal.SizeOf<SecurityCapabilities>();
                capabilities = Marshal.AllocHGlobal(capabilitySize);
                Marshal.StructureToPtr(new SecurityCapabilities { Package = sid }, capabilities, false);
                if (!UpdateProcThreadAttribute(attributes, 0, new IntPtr(0x20009), capabilities,
                    new IntPtr(capabilitySize), IntPtr.Zero, IntPtr.Zero))
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "set AppContainer identity");
                startup.Attributes = attributes;
                if (!CreateProcess(application, new StringBuilder(command), IntPtr.Zero, IntPtr.Zero,
                    false, 0x08080000, IntPtr.Zero, cwd, ref startup, out process))
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "launch " + application);
            } else {
                var systemReadSid = new SecurityIdentifier(backend.Contains("users") ? "S-1-5-32-545" : "S-1-15-2-1");
                var bytes = new byte[systemReadSid.BinaryLength];
                systemReadSid.GetBinaryForm(bytes, 0);
                var systemRead = Marshal.AllocHGlobal(bytes.Length);
                IntPtr logon = IntPtr.Zero;
                try {
                    Marshal.Copy(bytes, 0, systemRead, bytes.Length);
                    var restrictions = new List<SidAndAttributes> {
                        new SidAndAttributes { Sid = sid },
                        new SidAndAttributes { Sid = systemRead }
                    };
                    if (backend.Contains("logon")) {
                        using (var identity = WindowsIdentity.GetCurrent()) {
                            var group = LogonSid(identity.AccessToken.DangerousGetHandle());
                            var logonBytes = new byte[group.BinaryLength];
                            group.GetBinaryForm(logonBytes, 0);
                            logon = Marshal.AllocHGlobal(logonBytes.Length);
                            Marshal.Copy(logonBytes, 0, logon, logonBytes.Length);
                            restrictions.Add(new SidAndAttributes { Sid = logon });
                        }
                        if (logon == IntPtr.Zero) throw new Exception("Missing logon SID");
                    }
                    SafeAccessTokenHandle original;
                    if (!OpenProcessToken(GetCurrentProcess(), 0x000F01FF, out original))
                        throw new Win32Exception(Marshal.GetLastWin32Error(), "open process token");
                    using (original) {
                        SafeAccessTokenHandle token;
                        uint flags = backend.Contains("no-lua") ? 1u : 5u;
                        if (!CreateRestrictedToken(original.DangerousGetHandle(), flags,
                            0, IntPtr.Zero, 0, IntPtr.Zero, (uint)restrictions.Count, restrictions.ToArray(), out token))
                            throw new Win32Exception(Marshal.GetLastWin32Error(), "create full restricted token");
                        using (token) {
                            startup.Startup.Size = (uint)Marshal.SizeOf<StartupInfo>();
                            startup.Attributes = IntPtr.Zero;
                            if (!CreateProcessAsUser(token, application, new StringBuilder(command), IntPtr.Zero,
                                IntPtr.Zero, false, 0x08000000, IntPtr.Zero, cwd, ref startup, out process))
                                throw new Win32Exception(Marshal.GetLastWin32Error(), "launch full restricted token");
                        }
                    }
                } finally { Marshal.FreeHGlobal(systemRead); Marshal.FreeHGlobal(logon); }
            }
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

    public static void DescribeKernelSupport() {
        var module = LoadLibrary(Path.Combine(Environment.SystemDirectory, "processmodel.dll"));
        Console.WriteLine("PSEC DLL=" + (module != IntPtr.Zero));
        if (module == IntPtr.Zero) return;
        try {
            foreach (var name in new [] { "CreateProcessSecurityEnvironment", "QueryProcessSecurityEnvironmentSupport", "IsProcessSecurityEnvironmentVersionSupported", "CloseProcessSecurityEnvironment" })
                Console.WriteLine("PSEC EXPORT " + name + "=" + (GetProcAddress(module, name) != IntPtr.Zero));
            var query = GetProcAddress(module, "QueryProcessSecurityEnvironmentSupport");
            if (query != IntPtr.Zero) {
                ulong flags;
                int status = Marshal.GetDelegateForFunctionPointer<QuerySupport>(query)(out flags);
                Console.WriteLine("PSEC SUPPORT HRESULT=0x" + status.ToString("X8") + " FLAGS=0x" + flags.ToString("X"));
            }
            var version = GetProcAddress(module, "IsProcessSecurityEnvironmentVersionSupported");
            if (version != IntPtr.Zero) {
                byte available;
                uint minor;
                int status = Marshal.GetDelegateForFunctionPointer<QueryVersion>(version)(1, out available, out minor);
                Console.WriteLine("PSEC VERSION HRESULT=0x" + status.ToString("X8") + " AVAILABLE=" + available + " MINOR=" + minor);
            }
        } finally { FreeLibrary(module); }
    }

    public static void Run(string root, string shell, string shellName, string backend) {
        string profile = "CodexDeletionProbe" + Guid.NewGuid().ToString("N");
        IntPtr sid = IntPtr.Zero;
        Directory.CreateDirectory(root);
        try {
            if (backend == "appcontainer") {
                int result = CreateAppContainerProfile(profile, profile, "Temporary sandbox check", IntPtr.Zero, 0, out sid);
                if (result < 0) Marshal.ThrowExceptionForHR(result);
            } else {
                var unique = new SecurityIdentifier("S-1-5-21-10-20-30-" + BitConverter.ToUInt32(Guid.NewGuid().ToByteArray(), 0));
                var bytes = new byte[unique.BinaryLength];
                unique.GetBinaryForm(bytes, 0);
                sid = Marshal.AllocHGlobal(bytes.Length);
                Marshal.Copy(bytes, 0, sid, bytes.Length);
            }
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
            string readFile = Path.Combine(root, "read.txt");
            File.WriteAllText(readFile, "private read succeeded");
            string script = Path.Combine(root, shellName == "cmd" ? "probe.cmd" : "probe.ps1");
            if (shellName == "cmd") {
                File.WriteAllText(script, "@echo off\r\necho CWD=%CD%\r\ntype \"" + readFile + "\"\r\necho WRITE\r\necho wrote>\"" + writableFile + "\"\r\ntype \"" + writableFile + "\"\r\necho DELETE-ALLOWED\r\ndel \"" + writableFile + "\"\r\necho DELETE-OUTSIDE\r\ndel \"" + outsideFile + "\"\r\necho DELETE-PROTECTED\r\nrmdir \"" + protectedDir + "\"\r\ncmd /d /c echo child process succeeded\r\necho shell succeeded\r\n");
            } else {
                File.WriteAllText(script, "$ErrorActionPreference='Continue'\nWrite-Output ('CWD=' + (Get-Location))\n[IO.File]::ReadAllText('" + readFile.Replace("'", "''") + "')\nWrite-Output 'WRITE'\n[IO.File]::WriteAllText('" + writableFile.Replace("'", "''") + "', 'wrote')\n[IO.File]::ReadAllText('" + writableFile.Replace("'", "''") + "')\nWrite-Output 'DELETE-ALLOWED'\ntry { [IO.File]::Delete('" + writableFile.Replace("'", "''") + "') } catch { Write-Output $_.Exception.Message }\nWrite-Output 'DELETE-OUTSIDE'\ntry { [IO.File]::Delete('" + outsideFile.Replace("'", "''") + "') } catch { Write-Output $_.Exception.Message }\nWrite-Output 'DELETE-PROTECTED'\ntry { [IO.Directory]::Delete('" + protectedDir.Replace("'", "''") + "') } catch { Write-Output $_.Exception.Message }\n& cmd.exe /d /c 'echo child process succeeded'\nWrite-Output 'shell succeeded'\n");
            }
            string output = Path.Combine(workspace, "output.txt");
            string inner = shellName == "cmd" ? "\"" + script + "\"" : "\"" + shell + "\" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"" + script + "\"";
            string cmd = Path.Combine(Environment.SystemDirectory, "cmd.exe");
            uint exit = Start(sid, cmd, "\"" + cmd + "\" /d /s /c \"" + inner + " >\"" + output + "\" 2>&1\"", workspace, backend);
            string log = File.Exists(output) ? File.ReadAllText(output) : "NO OUTPUT FILE";
            Console.WriteLine("BACKEND=" + backend + " SHELL=" + shellName + " EXIT=" + exit + "\n" + log);
            bool writableDeleted = !File.Exists(writableFile), outsideKept = File.Exists(outsideFile), protectedKept = Directory.Exists(protectedDir);
            Console.WriteLine("RESULT writableDeleted=" + writableDeleted + " outsideKept=" + outsideKept + " protectedKept=" + protectedKept);
            if (exit != 0 || !log.Contains("private read succeeded") || !log.Contains("wrote") || !log.Contains("child process succeeded") || !log.Contains("shell succeeded") || !writableDeleted || !outsideKept || !protectedKept)
                throw new Exception(backend + " compatibility or deletion boundary check failed for " + shellName);
        } finally {
            if (sid != IntPtr.Zero) {
                if (backend == "appcontainer") { FreeSid(sid); DeleteAppContainerProfile(profile); }
                else Marshal.FreeHGlobal(sid);
            }
            Directory.Delete(root, true);
        }
    }
}
'@
Add-Type -TypeDefinition $source
[AppContainerCheck]::DescribeKernelSupport()
[AppContainerCheck]::InspectTokenCreation()
$failed = $false
$shells = @{
    cmd = (Join-Path $env:SystemRoot 'System32/cmd.exe')
    powershell = (Join-Path $env:SystemRoot 'System32/WindowsPowerShell/v1.0/powershell.exe')
    pwsh = (Get-Command pwsh).Source
}
foreach ($backend in @('appcontainer', 'full-restricted', 'full-restricted-logon', 'full-restricted-no-lua', 'full-restricted-users-logon')) {
    foreach ($entry in $shells.GetEnumerator()) {
        $root = Join-Path $env:GITHUB_WORKSPACE ('appcontainer-' + [Guid]::NewGuid().ToString('N'))
        Write-Host "CHECK backend=$backend shell=$($entry.Key)"
        try { [AppContainerCheck]::Run($root, $entry.Value, $entry.Key, $backend) }
        catch { $failed = $true; Write-Host $_.Exception.ToString() }
    }
}
if ($failed) { throw 'At least one AppContainer check failed' }
