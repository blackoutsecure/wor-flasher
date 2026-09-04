$source = @'
using System;
using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;

public static class Pi4Firmware
{
    [StructLayout(LayoutKind.Sequential)]
    private struct Luid { public uint LowPart; public int HighPart; }

    [StructLayout(LayoutKind.Sequential)]
    private struct TokenPrivileges { public uint Count; public Luid Luid; public uint Attributes; }

    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);

    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool LookupPrivilegeValue(string system, string name, out Luid luid);

    [DllImport("advapi32.dll", SetLastError = true)]
    private static extern bool AdjustTokenPrivileges(IntPtr token, bool disableAll, ref TokenPrivileges privileges, uint length, IntPtr previous, IntPtr required);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool SetFirmwareEnvironmentVariableEx(string name, string guid, byte[] value, uint size, uint attributes);

    public static void DisableRamLimit()
    {
        IntPtr token;
        Luid luid;
        if (!OpenProcessToken(Process.GetCurrentProcess().Handle, 0x28, out token) ||
            !LookupPrivilegeValue(null, "SeSystemEnvironmentPrivilege", out luid))
            throw new Win32Exception(Marshal.GetLastWin32Error());

        TokenPrivileges privileges = new TokenPrivileges { Count = 1, Luid = luid, Attributes = 2 };
        if (!AdjustTokenPrivileges(token, false, ref privileges, 0, IntPtr.Zero, IntPtr.Zero))
            throw new Win32Exception(Marshal.GetLastWin32Error());

        byte[] disabled = new byte[] { 0, 0, 0, 0 };
        if (!SetFirmwareEnvironmentVariableEx("RamLimitTo3GB", "{CD7CC258-31DB-22E6-9F22-63B0B8EED6B5}", disabled, 4, 7))
            throw new Win32Exception(Marshal.GetLastWin32Error());
    }
}
'@

Add-Type -TypeDefinition $source -Language CSharp
[Pi4Firmware]::DisableRamLimit()

#Also clear any BCD-level memory cap, which is separate from the UEFI firmware limit above.
if ((& bcdedit /enum '{current}') -match 'truncatememory') {
    & bcdedit /deletevalue '{current}' truncatememory | Out-Null
}
