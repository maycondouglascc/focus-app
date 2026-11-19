$ErrorActionPreference = 'SilentlyContinue'

[Console]::InputEncoding = New-Object System.Text.UTF8Encoding($false)
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)

$nativeCode = @'
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;

namespace FocusWin32
{
    public class WindowSample
    {
        public long hwnd;
        public int pid;
        public string exePath;
        public string fileDescription;
        public string productName;
        public string title;
    }

    public static class Native
    {
        private const uint PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;
        private static readonly Dictionary<string, string[]> VersionCache = new Dictionary<string, string[]>(StringComparer.OrdinalIgnoreCase);

        private delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

        [DllImport("user32.dll")]
        private static extern IntPtr GetForegroundWindow();

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        private static extern int GetWindowTextLengthW(IntPtr hWnd);

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        private static extern int GetWindowTextW(IntPtr hWnd, StringBuilder lpString, int nMaxCount);

        [DllImport("user32.dll")]
        private static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);

        [DllImport("user32.dll")]
        private static extern bool EnumChildWindows(IntPtr hWndParent, EnumWindowsProc lpEnumFunc, IntPtr lParam);

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        private static extern int GetClassNameW(IntPtr hWnd, StringBuilder lpClassName, int nMaxCount);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern IntPtr OpenProcess(uint dwDesiredAccess, bool bInheritHandle, uint dwProcessId);

        [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
        private static extern bool QueryFullProcessImageNameW(IntPtr hProcess, uint dwFlags, StringBuilder lpExeName, ref uint lpdwSize);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool CloseHandle(IntPtr hObject);

        public static string QueryProcessPath(uint pid)
        {
            if (pid == 0) return "";
            IntPtr h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
            if (h == IntPtr.Zero) return "";
            try
            {
                StringBuilder sb = new StringBuilder(1024);
                uint cap = (uint)sb.Capacity;
                if (QueryFullProcessImageNameW(h, 0, sb, ref cap) && cap > 0)
                {
                    return sb.ToString(0, (int)cap);
                }
                return "";
            }
            finally
            {
                CloseHandle(h);
            }
        }

        private static string GetWindowTitle(IntPtr hwnd)
        {
            int len = GetWindowTextLengthW(hwnd);
            if (len <= 0) return "";
            StringBuilder sb = new StringBuilder(len + 1);
            int read = GetWindowTextW(hwnd, sb, sb.Capacity);
            return read > 0 ? sb.ToString(0, read) : "";
        }

        private static string GetWindowClass(IntPtr hwnd)
        {
            StringBuilder sb = new StringBuilder(256);
            int read = GetClassNameW(hwnd, sb, sb.Capacity);
            return read > 0 ? sb.ToString(0, read) : "";
        }

        private static uint ResolveStoreAppChildPid(IntPtr parentHwnd, uint hostPid)
        {
            uint corePid = 0;
            uint fallbackPid = 0;
            EnumChildWindows(parentHwnd, delegate(IntPtr child, IntPtr lParam)
            {
                uint cpid = 0;
                GetWindowThreadProcessId(child, out cpid);
                if (cpid != 0 && cpid != hostPid)
                {
                    string cls = GetWindowClass(child);
                    if (string.Equals(cls, "Windows.UI.Core.CoreWindow", StringComparison.OrdinalIgnoreCase))
                    {
                        corePid = cpid;
                        return false;
                    }
                    if (fallbackPid == 0)
                    {
                        fallbackPid = cpid;
                    }
                }
                return true;
            }, IntPtr.Zero);
            return corePid != 0 ? corePid : fallbackPid;
        }

        private static string[] GetVersionStrings(string exePath)
        {
            if (string.IsNullOrEmpty(exePath)) return new string[] { "", "" };
            string[] cached;
            if (VersionCache.TryGetValue(exePath, out cached))
            {
                return cached;
            }
            string desc = "";
            string prod = "";
            try
            {
                FileVersionInfo info = FileVersionInfo.GetVersionInfo(exePath);
                if (info != null)
                {
                    desc = info.FileDescription ?? "";
                    prod = info.ProductName ?? "";
                }
            }
            catch
            {
                // Ignore inaccessible or non-versioned executables.
            }
            string[] pair = new string[] { desc.Trim(), prod.Trim() };
            if (VersionCache.Count > 512)
            {
                VersionCache.Clear();
            }
            VersionCache[exePath] = pair;
            return pair;
        }

        public static WindowSample GetForegroundSample()
        {
            IntPtr hwnd = GetForegroundWindow();
            if (hwnd == IntPtr.Zero) return null;

            uint pid = 0;
            GetWindowThreadProcessId(hwnd, out pid);
            if (pid == 0) return null;

            string exePath = QueryProcessPath(pid);
            string stem = "";
            if (!string.IsNullOrEmpty(exePath))
            {
                stem = Path.GetFileNameWithoutExtension(exePath) ?? "";
            }

            if (string.Equals(stem, "ApplicationFrameHost", StringComparison.OrdinalIgnoreCase))
            {
                uint childPid = ResolveStoreAppChildPid(hwnd, pid);
                if (childPid != 0)
                {
                    string childExe = QueryProcessPath(childPid);
                    if (!string.IsNullOrEmpty(childExe))
                    {
                        pid = childPid;
                        exePath = childExe;
                    }
                }
            }

            string[] ver = GetVersionStrings(exePath);
            WindowSample sample = new WindowSample();
            sample.hwnd = hwnd.ToInt64();
            sample.pid = unchecked((int)pid);
            sample.exePath = exePath ?? "";
            sample.fileDescription = ver[0];
            sample.productName = ver[1];
            sample.title = GetWindowTitle(hwnd);
            return sample;
        }
    }
}
'@

Add-Type -TypeDefinition $nativeCode -Language CSharp

$uiaReady = $false
try {
    $uiaCode = @'
using System;
using System.Collections.Generic;
using System.Windows.Automation;

namespace FocusWin32
{
    public static class BrowserUrl
    {
        private struct NodeEntry
        {
            public AutomationElement Element;
            public int Depth;
        }

        private static bool LooksLikeAddress(string value)
        {
            if (string.IsNullOrEmpty(value)) return false;
            string v = value.Trim();
            if (v.Length < 3 || v.Length > 2048) return false;
            if (v.IndexOf(' ') >= 0 || v.IndexOf('\n') >= 0 || v.IndexOf('\r') >= 0) return false;
            if (v.StartsWith("http://", StringComparison.OrdinalIgnoreCase) || v.StartsWith("https://", StringComparison.OrdinalIgnoreCase))
            {
                return true;
            }
            if (v.StartsWith("localhost:", StringComparison.OrdinalIgnoreCase) || string.Equals(v, "localhost", StringComparison.OrdinalIgnoreCase))
            {
                return true;
            }
            int dot = v.IndexOf('.');
            return dot > 0 && dot < v.Length - 1;
        }

        public static string ReadAddressBar(long hwndValue)
        {
            if (hwndValue == 0) return "";
            try
            {
                AutomationElement root = AutomationElement.FromHandle(new IntPtr(hwndValue));
                if (root == null) return "";

                TreeWalker walker = TreeWalker.ControlViewWalker;
                Queue<NodeEntry> queue = new Queue<NodeEntry>();
                NodeEntry start = new NodeEntry();
                start.Element = root;
                start.Depth = 0;
                queue.Enqueue(start);

                int visited = 0;
                string fallback = "";

                while (queue.Count > 0 && visited < 240)
                {
                    NodeEntry cur = queue.Dequeue();
                    visited++;

                    ControlType ct = null;
                    try
                    {
                        ct = cur.Element.Current.ControlType;
                    }
                    catch
                    {
                        continue;
                    }

                    // Never descend into web page DOM documents.
                    if (ct == ControlType.Document)
                    {
                        continue;
                    }

                    if (ct == ControlType.Edit)
                    {
                        try
                        {
                            object patternObj = null;
                            if (cur.Element.TryGetCurrentPattern(ValuePattern.Pattern, out patternObj))
                            {
                                ValuePattern vp = patternObj as ValuePattern;
                                if (vp != null)
                                {
                                    string val = (vp.Current.Value ?? "").Trim();
                                    string aid = cur.Element.Current.AutomationId ?? "";
                                    if (string.Equals(aid, "view_1012", StringComparison.OrdinalIgnoreCase) ||
                                        string.Equals(aid, "urlbar-input", StringComparison.OrdinalIgnoreCase))
                                    {
                                        return val;
                                    }
                                    if (string.IsNullOrEmpty(fallback) && LooksLikeAddress(val))
                                    {
                                        fallback = val;
                                    }
                                }
                            }
                        }
                        catch
                        {
                            // Ignore transient UIA failures on individual controls.
                        }
                        continue;
                    }

                    if (cur.Depth >= 8)
                    {
                        continue;
                    }

                    AutomationElement child = null;
                    try
                    {
                        child = walker.GetFirstChild(cur.Element);
                    }
                    catch
                    {
                        child = null;
                    }

                    while (child != null)
                    {
                        NodeEntry next = new NodeEntry();
                        next.Element = child;
                        next.Depth = cur.Depth + 1;
                        queue.Enqueue(next);
                        try
                        {
                            child = walker.GetNextSibling(child);
                        }
                        catch
                        {
                            break;
                        }
                    }
                }

                return fallback;
            }
            catch
            {
                return "";
            }
        }
    }
}
'@
    Add-Type -TypeDefinition $uiaCode -ReferencedAssemblies @('UIAutomationClient', 'UIAutomationTypes', 'WindowsBase') -Language CSharp
    $uiaReady = $true
} catch {
    $uiaReady = $false
}

$audioReady = $false
try {
    $audioCode = @'
using System;
using System.Runtime.InteropServices;

namespace FocusWin32
{
    public static class AudioCheck
    {
        private const int eRender = 0;
        private const int eMultimedia = 1;
        private const uint CLSCTX_ALL = 23;
        private const int AudioSessionStateActive = 1;
        private const uint PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern IntPtr OpenProcess(uint dwDesiredAccess, bool bInheritHandle, uint dwProcessId);

        [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
        private static extern bool QueryFullProcessImageNameW(IntPtr hProcess, uint dwFlags, System.Text.StringBuilder lpExeName, ref uint lpdwSize);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool CloseHandle(IntPtr hObject);

        private static string QueryProcessPath(uint pid)
        {
            if (pid == 0) return "";
            IntPtr h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
            if (h == IntPtr.Zero) return "";
            try
            {
                System.Text.StringBuilder sb = new System.Text.StringBuilder(1024);
                uint cap = (uint)sb.Capacity;
                if (QueryFullProcessImageNameW(h, 0, sb, ref cap) && cap > 0)
                {
                    return sb.ToString(0, (int)cap);
                }
                return "";
            }
            finally
            {
                CloseHandle(h);
            }
        }

        [ComImport]
        [Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")]
        private class MMDeviceEnumeratorComObject
        {
        }

        [Guid("A95664D2-9614-4F35-A746-DE8DB63617E6")]
        [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        private interface IMMDeviceEnumerator
        {
            int NotImpl1();
            [PreserveSig]
            int GetDefaultAudioEndpoint(int dataFlow, int role, out IMMDevice ppDevice);
        }

        [Guid("D666063F-1587-4E43-81F1-B948E807363F")]
        [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        private interface IMMDevice
        {
            [PreserveSig]
            int Activate(ref Guid iid, uint dwClsCtx, IntPtr pActivationParams, [MarshalAs(UnmanagedType.IUnknown)] out object ppInterface);
        }

        [Guid("77AA99A0-1BD6-484F-8BC7-2C654C9A9B6F")]
        [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        private interface IAudioSessionManager2
        {
            int NotImpl1();
            int NotImpl2();
            [PreserveSig]
            int GetSessionEnumerator(out IAudioSessionEnumerator SessionEnum);
        }

        [Guid("E2F5BB11-0570-40CA-ACDD-3AA01277DEE8")]
        [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        private interface IAudioSessionEnumerator
        {
            [PreserveSig]
            int GetCount(out int SessionCount);
            [PreserveSig]
            int GetSession(int SessionCount, out IAudioSessionControl Session);
        }

        [Guid("F4B1A599-7266-4319-A8CA-E70ACB11E8CD")]
        [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        private interface IAudioSessionControl
        {
            [PreserveSig]
            int GetState(out int pRetVal);
        }

        [Guid("bfb7ff88-7239-4fc9-8fa2-07c950be9c6d")]
        [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
        private interface IAudioSessionControl2
        {
            [PreserveSig]
            int GetState(out int pRetVal);
            [PreserveSig]
            int GetDisplayName([MarshalAs(UnmanagedType.LPWStr)] out string pRetVal);
            [PreserveSig]
            int SetDisplayName([MarshalAs(UnmanagedType.LPWStr)] string Value, [MarshalAs(UnmanagedType.LPStruct)] Guid EventContext);
            [PreserveSig]
            int GetIconPath([MarshalAs(UnmanagedType.LPWStr)] out string pRetVal);
            [PreserveSig]
            int SetIconPath([MarshalAs(UnmanagedType.LPWStr)] string Value, [MarshalAs(UnmanagedType.LPStruct)] Guid EventContext);
            [PreserveSig]
            int GetGroupingParam(out Guid pRetVal);
            [PreserveSig]
            int SetGroupingParam([MarshalAs(UnmanagedType.LPStruct)] Guid Override, [MarshalAs(UnmanagedType.LPStruct)] Guid EventContext);
            int NotImpl1();
            int NotImpl2();
            [PreserveSig]
            int GetSessionIdentifier([MarshalAs(UnmanagedType.LPWStr)] out string pRetVal);
            [PreserveSig]
            int GetSessionInstanceIdentifier([MarshalAs(UnmanagedType.LPWStr)] out string pRetVal);
            [PreserveSig]
            int GetProcessId(out uint pRetVal);
        }

        public static bool IsProcessPlayingAudio(int targetPid, string targetExePath)
        {
            IMMDeviceEnumerator enumerator = null;
            IMMDevice device = null;
            object mgrObj = null;
            IAudioSessionManager2 manager = null;
            IAudioSessionEnumerator sessions = null;

            try
            {
                enumerator = (IMMDeviceEnumerator)(new MMDeviceEnumeratorComObject());
                if (enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia, out device) != 0 || device == null)
                {
                    return false;
                }

                Guid iid = typeof(IAudioSessionManager2).GUID;
                if (device.Activate(ref iid, CLSCTX_ALL, IntPtr.Zero, out mgrObj) != 0 || mgrObj == null)
                {
                    return false;
                }

                manager = (IAudioSessionManager2)mgrObj;
                if (manager.GetSessionEnumerator(out sessions) != 0 || sessions == null)
                {
                    return false;
                }

                int count = 0;
                if (sessions.GetCount(out count) != 0 || count <= 0)
                {
                    return false;
                }

                string expectedExe = targetExePath ?? "";
                for (int i = 0; i < count; i++)
                {
                    IAudioSessionControl ctrl = null;
                    try
                    {
                        if (sessions.GetSession(i, out ctrl) != 0 || ctrl == null) continue;
                        int state = 0;
                        if (ctrl.GetState(out state) != 0 || state != AudioSessionStateActive) continue;

                        IAudioSessionControl2 ctrl2 = ctrl as IAudioSessionControl2;
                        if (ctrl2 == null) continue;

                        uint spid = 0;
                        if (ctrl2.GetProcessId(out spid) != 0 || spid == 0) continue;

                        if (targetPid > 0 && spid == (uint)targetPid)
                        {
                            return true;
                        }

                        if (!string.IsNullOrEmpty(expectedExe))
                        {
                            string sExe = QueryProcessPath(spid);
                            if (!string.IsNullOrEmpty(sExe) && string.Equals(sExe, expectedExe, StringComparison.OrdinalIgnoreCase))
                            {
                                return true;
                            }
                        }
                    }
                    finally
                    {
                        if (ctrl != null) Marshal.ReleaseComObject(ctrl);
                    }
                }
                return false;
            }
            catch
            {
                return false;
            }
            finally
            {
                if (sessions != null) Marshal.ReleaseComObject(sessions);
                if (manager != null) Marshal.ReleaseComObject(manager);
                if (device != null) Marshal.ReleaseComObject(device);
                if (enumerator != null) Marshal.ReleaseComObject(enumerator);
            }
        }
    }
}
'@
    Add-Type -TypeDefinition $audioCode -Language CSharp
    $audioReady = $true
} catch {
    $audioReady = $false
}

$browserStems = @{
    'chrome' = $true; 'msedge' = $true; 'brave' = $true; 'firefox' = $true;
    'vivaldi' = $true; 'opera' = $true; 'arc' = $true; 'chromium' = $true;
    'thorium' = $true; 'zen' = $true; 'librewolf' = $true; 'floorp' = $true;
    'waterfox' = $true; 'comet' = $true; 'dia' = $true; 'duckduckgo' = $true;
    'yandex' = $true; 'qutebrowser' = $true
}

function Write-JsonLine($obj) {
    $json = ConvertTo-Json -InputObject $obj -Compress -Depth 4
    [Console]::Out.WriteLine($json)
    [Console]::Out.Flush()
}

Write-JsonLine @{ ready = $true; uia = $uiaReady; audio = $audioReady }

while ($true) {
    $line = [Console]::In.ReadLine()
    if ($null -eq $line) { break }
    $trimmed = $line.Trim()
    if (-not $trimmed) { continue }

    $req = $null
    try {
        $req = ConvertFrom-Json -InputObject $trimmed
    } catch {
        continue
    }

    $id = $req.id
    $cmd = [string]$req.cmd

    if ($cmd -eq 'quit') {
        break
    } elseif ($cmd -eq 'ping') {
        Write-JsonLine @{ id = $id; ok = $true; data = @{ uia = $uiaReady; audio = $audioReady } }
    } elseif ($cmd -eq 'sample') {
        try {
            $s = [FocusWin32.Native]::GetForegroundSample()
            if ($null -eq $s) {
                Write-JsonLine @{ id = $id; ok = $true; data = $null }
            } else {
                $url = ''
                if ($req.browserTabs -and $uiaReady -and $s.hwnd -ne 0 -and $s.exePath) {
                    $stem = [System.IO.Path]::GetFileNameWithoutExtension($s.exePath).ToLowerInvariant()
                    if ($browserStems.ContainsKey($stem)) {
                        $url = [FocusWin32.BrowserUrl]::ReadAddressBar($s.hwnd)
                    }
                }
                Write-JsonLine @{
                    id = $id
                    ok = $true
                    data = @{
                        hwnd = $s.hwnd
                        pid = $s.pid
                        exePath = $s.exePath
                        fileDescription = $s.fileDescription
                        productName = $s.productName
                        title = $s.title
                        url = $url
                    }
                }
            }
        } catch {
            Write-JsonLine @{ id = $id; ok = $false; error = $_.Exception.Message }
        }
    } elseif ($cmd -eq 'media') {
        try {
            $active = $false
            if ($audioReady) {
                $pidVal = 0
                if ($null -ne $req.pid) { $pidVal = [int]$req.pid }
                $exeVal = ''
                if ($null -ne $req.exePath) { $exeVal = [string]$req.exePath }
                $active = [FocusWin32.AudioCheck]::IsProcessPlayingAudio($pidVal, $exeVal)
            }
            Write-JsonLine @{ id = $id; ok = $true; data = @{ active = $active } }
        } catch {
            Write-JsonLine @{ id = $id; ok = $true; data = @{ active = $false } }
        }
    } else {
        Write-JsonLine @{ id = $id; ok = $false; error = 'unknown-cmd' }
    }
}
