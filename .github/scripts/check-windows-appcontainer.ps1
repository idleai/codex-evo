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
    [StructLayout(LayoutKind.Sequential)]
    private struct Luid { public uint Low; public int High; }
    [StructLayout(LayoutKind.Sequential)]
    private struct TokenPrivileges { public uint Count; public Luid Luid; public uint Attributes; }
    [StructLayout(LayoutKind.Sequential)]
    private struct UnicodeString { public ushort Length, MaximumLength; public IntPtr Buffer; }
    [StructLayout(LayoutKind.Sequential)]
    private struct ObjectAttributes {
        public uint Length;
        public IntPtr Root, Name;
        public uint Attributes;
        public IntPtr SecurityDescriptor, SecurityQualityOfService;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct IoStatus { public IntPtr Status; public UIntPtr Information; }
    [DllImport("ntdll.dll")]
    private static extern int NtOpenFile(out IntPtr handle, uint access,
        ref ObjectAttributes attributes, out IoStatus status, uint share, uint options);
    [DllImport("ntdll.dll")]
    private static extern int NtOpenDirectoryObject(out IntPtr handle, uint access,
        ref ObjectAttributes attributes);
    [DllImport("ntdll.dll")]
    private static extern int NtQuerySecurityObject(IntPtr handle, uint information,
        [Out] byte[] buffer, uint size, out uint needed);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool LookupPrivilegeValue(string system, string name, out Luid luid);
    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool AdjustTokenPrivileges(SafeAccessTokenHandle token, bool disableAll,
        ref TokenPrivileges state, uint length, IntPtr previous, IntPtr returned);
    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool GetTokenInformation(IntPtr token, int kind,
        IntPtr buffer, uint size, out uint required);
    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool SetTokenInformation(SafeAccessTokenHandle token, int kind,
        IntPtr buffer, uint size);
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

    private static void InspectSystemObject(string path, uint access, bool directory, bool describeAcl) {
        var buffer = Marshal.StringToHGlobalUni(path);
        var name = Marshal.AllocHGlobal(Marshal.SizeOf<UnicodeString>());
        IntPtr handle = IntPtr.Zero;
        try {
            Marshal.StructureToPtr(new UnicodeString {
                Length = (ushort)(path.Length * 2), MaximumLength = (ushort)((path.Length + 1) * 2), Buffer = buffer
            }, name, false);
            var attributes = new ObjectAttributes {
                Length = (uint)Marshal.SizeOf<ObjectAttributes>(), Name = name, Attributes = 0x40
            };
            IoStatus io;
            int status = directory ? NtOpenDirectoryObject(out handle, access, ref attributes)
                : NtOpenFile(out handle, access, ref attributes, out io, 7, 0);
            Console.WriteLine("OBJECT " + path + " ACCESS=0x" + access.ToString("X8") + " STATUS=0x" + status.ToString("X8"));
            if (status < 0 || !describeAcl) return;
            uint needed;
            NtQuerySecurityObject(handle, 4, null, 0, out needed);
            if (needed == 0 || needed > 65536) {
                Console.WriteLine("OBJECT DACL size=" + needed);
                return;
            }
            var descriptor = new byte[needed];
            status = NtQuerySecurityObject(handle, 4, descriptor, needed, out needed);
            Console.WriteLine("OBJECT DACL STATUS=0x" + status.ToString("X8"));
            if (status >= 0)
                Console.WriteLine("OBJECT DACL " + new RawSecurityDescriptor(descriptor, 0).GetSddlForm(AccessControlSections.Access));
        } finally {
            if (handle != IntPtr.Zero) CloseHandle(handle);
            Marshal.FreeHGlobal(name);
            Marshal.FreeHGlobal(buffer);
        }
    }

    public static void InspectSystemObjects() {
        foreach (var path in new [] { @"\Device\KsecDD", @"\Device\CNG" })
            InspectSystemObject(path, 0x20000, false, true);
        InspectSystemObject(@"\KnownDlls", 0x20000, true, true);
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

    private static void LabelTemporaryDirectoryLow(string path) {
        var start = new System.Diagnostics.ProcessStartInfo("icacls.exe") {
            UseShellExecute = false, RedirectStandardOutput = true, RedirectStandardError = true
        };
        start.ArgumentList.Add(path);
        start.ArgumentList.Add("/setintegritylevel");
        start.ArgumentList.Add("(OI)(CI)L");
        using (var process = System.Diagnostics.Process.Start(start)) {
            var output = process.StandardOutput.ReadToEnd();
            var error = process.StandardError.ReadToEnd();
            process.WaitForExit();
            if (process.ExitCode != 0) throw new Exception("Temporary label failed: " + output + error);
        }
    }

    private static uint Start(IntPtr sid, string application, string command, string cwd, string backend) {
        IntPtr attributes = IntPtr.Zero, capabilities = IntPtr.Zero;
        bool initialized = false;
        try {
            var startup = new StartupInfoEx();
            startup.Startup.Size = (uint)Marshal.SizeOf<StartupInfoEx>();
            startup.Startup.Desktop = @"winsta0\default";
            ProcessInfo process;
            if (backend.StartsWith("appcontainer")) {
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
                var systemReadSid = new SecurityIdentifier(backend.Contains("world") ? "S-1-1-0" : "S-1-5-32-545");
                var bytes = new byte[systemReadSid.BinaryLength];
                systemReadSid.GetBinaryForm(bytes, 0);
                var systemRead = Marshal.AllocHGlobal(bytes.Length);
                IntPtr logon = IntPtr.Zero, restrictedCode = IntPtr.Zero, applicationGroup = IntPtr.Zero;
                try {
                    Marshal.Copy(bytes, 0, systemRead, bytes.Length);
                    var restrictions = new List<SidAndAttributes> {
                        new SidAndAttributes { Sid = sid }
                    };
                    if (backend.Contains("users") || backend.Contains("world"))
                        restrictions.Add(new SidAndAttributes { Sid = systemRead });
                    if (backend.Contains("restricted-code")) {
                        var group = new SecurityIdentifier("S-1-5-12");
                        var groupBytes = new byte[group.BinaryLength];
                        group.GetBinaryForm(groupBytes, 0);
                        restrictedCode = Marshal.AllocHGlobal(groupBytes.Length);
                        Marshal.Copy(groupBytes, 0, restrictedCode, groupBytes.Length);
                        restrictions.Add(new SidAndAttributes { Sid = restrictedCode });
                    }
                    if (backend.Contains("application-group")) {
                        var group = new SecurityIdentifier(backend.Contains("restricted-application-group") ? "S-1-15-2-2" : "S-1-15-2-1");
                        var groupBytes = new byte[group.BinaryLength];
                        group.GetBinaryForm(groupBytes, 0);
                        applicationGroup = Marshal.AllocHGlobal(groupBytes.Length);
                        Marshal.Copy(groupBytes, 0, applicationGroup, groupBytes.Length);
                        restrictions.Add(new SidAndAttributes { Sid = applicationGroup });
                    }
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
                        if (backend.StartsWith("write-restricted")) flags |= 8u;
                        if (!CreateRestrictedToken(original.DangerousGetHandle(), flags,
                            0, IntPtr.Zero, 0, IntPtr.Zero, (uint)restrictions.Count, restrictions.ToArray(), out token))
                            throw new Win32Exception(Marshal.GetLastWin32Error(), "create full restricted token");
                        using (token) {
                            Luid traverse;
                            if (!LookupPrivilegeValue(null, "SeChangeNotifyPrivilege", out traverse))
                                throw new Win32Exception(Marshal.GetLastWin32Error(), "lookup traversal privilege");
                            var privileges = new TokenPrivileges { Count = 1, Luid = traverse, Attributes = 2 };
                            if (!AdjustTokenPrivileges(token, false, ref privileges, 0, IntPtr.Zero, IntPtr.Zero))
                                throw new Win32Exception(Marshal.GetLastWin32Error(), "enable traversal privilege");
                            int privilegeError = Marshal.GetLastWin32Error();
                            if (privilegeError != 0) throw new Win32Exception(privilegeError, "assign traversal privilege");
                            if (backend.Contains("low-integrity")) {
                                var lowSid = new SecurityIdentifier("S-1-16-4096");
                                var lowBytes = new byte[lowSid.BinaryLength];
                                lowSid.GetBinaryForm(lowBytes, 0);
                                int labelSize = Marshal.SizeOf<SidAndAttributes>();
                                var label = Marshal.AllocHGlobal(labelSize + lowBytes.Length);
                                try {
                                    var lowSidPointer = IntPtr.Add(label, labelSize);
                                    Marshal.Copy(lowBytes, 0, lowSidPointer, lowBytes.Length);
                                    Marshal.StructureToPtr(new SidAndAttributes { Sid = lowSidPointer, Attributes = 0x20 }, label, false);
                                    if (!SetTokenInformation(token, 25, label, (uint)(labelSize + lowBytes.Length)))
                                        throw new Win32Exception(Marshal.GetLastWin32Error(), "lower process integrity");
                                } finally { Marshal.FreeHGlobal(label); }
                            }
                            var defaultAcl = new RawAcl(2, 1);
                            defaultAcl.InsertAce(0, new CommonAce(AceFlags.None, AceQualifier.AccessAllowed,
                                0x10000000, new SecurityIdentifier(logon), false, null));
                            var defaultAclBytes = new byte[defaultAcl.BinaryLength];
                            defaultAcl.GetBinaryForm(defaultAclBytes, 0);
                            var defaultAclBuffer = Marshal.AllocHGlobal(defaultAclBytes.Length);
                            var defaultAclInfo = Marshal.AllocHGlobal(IntPtr.Size);
                            try {
                                Marshal.Copy(defaultAclBytes, 0, defaultAclBuffer, defaultAclBytes.Length);
                                Marshal.WriteIntPtr(defaultAclInfo, defaultAclBuffer);
                                if (!SetTokenInformation(token, 6, defaultAclInfo, (uint)IntPtr.Size))
                                    throw new Win32Exception(Marshal.GetLastWin32Error(), "set logon default DACL");
                            } finally {
                                Marshal.FreeHGlobal(defaultAclInfo);
                                Marshal.FreeHGlobal(defaultAclBuffer);
                            }
                            if (backend.Contains("restricted-code")) {
                                WindowsIdentity.RunImpersonated(token, () => {
                                    foreach (var path in new [] { @"\Device\KsecDD", @"\Device\CNG" })
                                        InspectSystemObject(path, 0xC0000000, false, false);
                                    InspectSystemObject(@"\KnownDlls", 1, true, false);
                                });
                            }
                            startup.Startup.Size = (uint)Marshal.SizeOf<StartupInfo>();
                            startup.Attributes = IntPtr.Zero;
                            uint creationFlags = backend.Contains("inherit-console") ? 0u : 0x08000000u;
                            if (!CreateProcessAsUser(token, application, new StringBuilder(command), IntPtr.Zero,
                                IntPtr.Zero, false, creationFlags, IntPtr.Zero, cwd, ref startup, out process))
                                throw new Win32Exception(Marshal.GetLastWin32Error(), "launch full restricted token");
                        }
                    }
                } finally {
                    Marshal.FreeHGlobal(systemRead);
                    Marshal.FreeHGlobal(logon);
                    Marshal.FreeHGlobal(restrictedCode);
                    Marshal.FreeHGlobal(applicationGroup);
                }
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
            if (backend.StartsWith("appcontainer")) {
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
            if (backend.Contains("ancestor-metadata")) {
                for (var parent = Directory.GetParent(root); parent != null; parent = parent.Parent) {
                    var parentAcl = parent.GetAccessControl();
                    parentAcl.AddAccessRule(new FileSystemAccessRule(package,
                        FileSystemRights.ReadAttributes | FileSystemRights.Traverse | FileSystemRights.ReadPermissions | FileSystemRights.Synchronize,
                        InheritanceFlags.None, PropagationFlags.None, AccessControlType.Allow));
                    parent.SetAccessControl(parentAcl);
                }
            }
            string workspace = Path.Combine(root, "workspace"), outside = Path.Combine(root, "outside");
            Directory.CreateDirectory(workspace);
            Directory.CreateDirectory(outside);
            var writeAcl = new DirectoryInfo(workspace).GetAccessControl();
            writeAcl.AddAccessRule(new FileSystemAccessRule(package, FileSystemRights.Modify,
                inheritance, PropagationFlags.None, AccessControlType.Allow));
            if (backend.Contains("parent-guard")) {
                var guard = backend.Contains("all-app-guard") ? new SecurityIdentifier("S-1-15-2-1") : new SecurityIdentifier(WellKnownSidType.WorldSid, null);
                writeAcl.AddAccessRule(new FileSystemAccessRule(guard,
                    FileSystemRights.DeleteSubdirectoriesAndFiles, InheritanceFlags.ContainerInherit,
                    PropagationFlags.None, AccessControlType.Deny));
            }
            new DirectoryInfo(workspace).SetAccessControl(writeAcl);
            string otherLowRoot = Path.Combine(root, "other-low-workspace");
            Directory.CreateDirectory(otherLowRoot);
            if (backend.Contains("low-integrity")) {
                LabelTemporaryDirectoryLow(workspace);
                LabelTemporaryDirectoryLow(otherLowRoot);
                var otherAcl = new DirectoryInfo(otherLowRoot).GetAccessControl();
                otherAcl.AddAccessRule(new FileSystemAccessRule(new SecurityIdentifier(WellKnownSidType.WorldSid, null),
                    FileSystemRights.DeleteSubdirectoriesAndFiles, InheritanceFlags.ContainerInherit,
                    PropagationFlags.None, AccessControlType.Deny));
                new DirectoryInfo(otherLowRoot).SetAccessControl(otherAcl);
            }
            string otherLowFile = Path.Combine(otherLowRoot, "delete.txt");
            File.WriteAllText(otherLowFile, "another workspace");
            var protectedDir = Path.Combine(workspace, ".git");
            Directory.CreateDirectory(protectedDir);
            var deniedAcl = new DirectoryInfo(protectedDir).GetAccessControl();
            deniedAcl.AddAccessRule(new FileSystemAccessRule(package, FileSystemRights.Write | FileSystemRights.Delete,
                inheritance, PropagationFlags.None, AccessControlType.Deny));
            if (backend.Contains("all-app-guard")) {
                deniedAcl.AddAccessRule(new FileSystemAccessRule(new SecurityIdentifier("S-1-15-2-1"),
                    FileSystemRights.Write | FileSystemRights.Delete,
                    inheritance, PropagationFlags.None, AccessControlType.Deny));
            }
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
            if (shellName == "cmd") {
                File.AppendAllText(script, "echo DELETE-OTHER-LOW\r\ndel \"" + otherLowFile + "\"\r\nexit /b 0\r\n");
            } else {
                File.AppendAllText(script, "Write-Output 'DELETE-OTHER-LOW'\ntry { [IO.File]::Delete('" + otherLowFile.Replace("'", "''") + "') } catch { Write-Output $_.Exception.Message }\n");
            }
            if (backend.Contains("low-integrity")) {
                string privateTemp = Path.Combine(workspace, "temp");
                Directory.CreateDirectory(privateTemp);
                inner = "set \"TEMP=" + privateTemp + "\" && set \"TMP=" + privateTemp + "\" && " + inner;
            }
            string cmd = Path.Combine(Environment.SystemDirectory, "cmd.exe");
            uint exit = Start(sid, cmd, "\"" + cmd + "\" /d /s /c \"" + inner + " >\"" + output + "\" 2>&1\"", workspace, backend);
            string log = File.Exists(output) ? File.ReadAllText(output) : "NO OUTPUT FILE";
            Console.WriteLine("BACKEND=" + backend + " SHELL=" + shellName + " EXIT=" + exit + "\n" + log);
            bool writableDeleted = !File.Exists(writableFile), outsideKept = File.Exists(outsideFile), protectedKept = Directory.Exists(protectedDir);
            bool otherLowKept = File.Exists(otherLowFile), cwdCorrect = log.Contains("CWD=" + workspace, StringComparison.OrdinalIgnoreCase);
            Console.WriteLine("RESULT writableDeleted=" + writableDeleted + " outsideKept=" + outsideKept + " protectedKept=" + protectedKept + " otherLowKept=" + otherLowKept + " cwdCorrect=" + cwdCorrect);
            if (exit != 0 || !log.Contains("private read succeeded") || !log.Contains("wrote") || !log.Contains("child process succeeded") || !log.Contains("shell succeeded") || !writableDeleted || !outsideKept || !protectedKept || !otherLowKept || !cwdCorrect)
                throw new Exception(backend + " compatibility or deletion boundary check failed for " + shellName);
        } finally {
            if (sid != IntPtr.Zero) {
                if (backend.Contains("ancestor-metadata")) {
                    var package = new SecurityIdentifier(sid);
                    for (var parent = Directory.GetParent(root); parent != null; parent = parent.Parent) {
                        var parentAcl = parent.GetAccessControl();
                        parentAcl.PurgeAccessRules(package);
                        parent.SetAccessControl(parentAcl);
                    }
                }
                if (backend.StartsWith("appcontainer")) { FreeSid(sid); DeleteAppContainerProfile(profile); }
                else Marshal.FreeHGlobal(sid);
            }
            Directory.Delete(root, true);
        }
    }
}
'@
Add-Type -TypeDefinition $source
[AppContainerCheck]::DescribeKernelSupport()
[AppContainerCheck]::InspectSystemObjects()
[AppContainerCheck]::InspectTokenCreation()
$failed = $false
$shells = @{
    cmd = (Join-Path $env:SystemRoot 'System32/cmd.exe')
    powershell = (Join-Path $env:SystemRoot 'System32/WindowsPowerShell/v1.0/powershell.exe')
    pwsh = (Get-Command pwsh).Source
}
foreach ($backend in @('appcontainer-parent-guard-all-app-guard-ancestor-metadata', 'full-restricted-users-logon', 'full-restricted-users-logon-inherit-console', 'full-restricted-users-logon-no-lua', 'full-restricted-world-logon', 'write-restricted-logon', 'write-restricted-logon-low-integrity-parent-guard-inherit-console', 'write-restricted-world-logon-low-integrity-parent-guard-inherit-console', 'full-restricted-users-restricted-code-logon-parent-guard', 'full-restricted-users-restricted-code-logon-low-integrity-parent-guard', 'write-restricted-code-logon-low-integrity-parent-guard-inherit-console', 'full-restricted-users-restricted-code-application-group-logon-parent-guard', 'full-restricted-users-restricted-code-restricted-application-group-logon-parent-guard')) {
    foreach ($entry in $shells.GetEnumerator()) {
        $root = Join-Path $env:GITHUB_WORKSPACE ('appcontainer-' + [Guid]::NewGuid().ToString('N'))
        Write-Host "CHECK backend=$backend shell=$($entry.Key)"
        try { [AppContainerCheck]::Run($root, $entry.Value, $entry.Key, $backend) }
        catch { $failed = $true; Write-Host $_.Exception.ToString() }
    }
}
if ($failed) { throw 'At least one AppContainer check failed' }
