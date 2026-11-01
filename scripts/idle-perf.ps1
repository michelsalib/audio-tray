# Idle cost of a running audio-tray: CPU cycles and context switches (wakeups) per thread over a
# window, the numbers Process Explorer shows as "Cycles Delta" and "Context Switch Delta". No
# elevation needed, unlike WPR. Usage: scripts\idle-perf.ps1 [-Seconds 60] [-Name audio-tray | -Id pid]
param([int]$Seconds = 60, [string]$Name = 'audio-tray', [int]$Id = 0)

Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

public static class IdlePerf {
    [DllImport("ntdll.dll")] static extern int NtQuerySystemInformation(int cls, IntPtr buf, int len, out int ret);
    [DllImport("kernel32.dll")] static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
    [DllImport("kernel32.dll")] static extern IntPtr OpenThread(uint access, bool inherit, uint tid);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll")] static extern bool QueryProcessCycleTime(IntPtr h, out ulong cycles);
    [DllImport("kernel32.dll")] static extern bool QueryThreadCycleTime(IntPtr h, out ulong cycles);
    [DllImport("kernel32.dll")] static extern int GetThreadDescription(IntPtr h, out IntPtr desc);
    [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr p);
    [DllImport("ntdll.dll")] static extern int NtQueryInformationThread(IntPtr h, int cls, out IntPtr info, int len, IntPtr ret);

    const uint PROCESS_QUERY_LIMITED = 0x1000;
    const uint THREAD_QUERY_LIMITED = 0x0800;

    public class Thread { public uint Id; public string Name; public uint Switches; public ulong Cycles; public long Start; }

    public static ulong ProcessCycles(uint pid) {
        IntPtr h = OpenProcess(PROCESS_QUERY_LIMITED, false, pid);
        ulong c = 0;
        if (h != IntPtr.Zero) { QueryProcessCycleTime(h, out c); CloseHandle(h); }
        return c;
    }

    // SystemProcessInformation (5): SYSTEM_PROCESS_INFORMATION (0x100 bytes on x64), then one
    // SYSTEM_THREAD_INFORMATION (0x50) per thread: UniqueThread at 0x30, ContextSwitches at 0x40.
    public static List<Thread> Threads(uint pid) {
        int len = 1 << 20;
        IntPtr buf = IntPtr.Zero;
        try {
            while (true) {
                buf = Marshal.AllocHGlobal(len);
                int ret;
                int status = NtQuerySystemInformation(5, buf, len, out ret);
                if (status == 0) break;
                Marshal.FreeHGlobal(buf); buf = IntPtr.Zero;
                if ((uint)status != 0xC0000004) throw new Exception("NtQuerySystemInformation 0x" + status.ToString("X"));
                len = Math.Max(len * 2, ret + 65536);
            }
            var list = new List<Thread>();
            long offset = 0;
            while (true) {
                IntPtr p = new IntPtr(buf.ToInt64() + offset);
                int next = Marshal.ReadInt32(p, 0);
                int count = Marshal.ReadInt32(p, 4);
                if ((uint)Marshal.ReadIntPtr(p, 0x50).ToInt64() == pid) {
                    for (int i = 0; i < count; i++) {
                        IntPtr t = new IntPtr(p.ToInt64() + 0x100 + i * 0x50);
                        var th = new Thread {
                            Id = (uint)Marshal.ReadIntPtr(t, 0x30).ToInt64(),
                            Switches = (uint)Marshal.ReadInt32(t, 0x40),
                        };
                        // THREAD_QUERY_INFORMATION too, for the start address (class 9).
                        IntPtr h = OpenThread(THREAD_QUERY_LIMITED | 0x0040, false, th.Id);
                        if (h != IntPtr.Zero) {
                            QueryThreadCycleTime(h, out th.Cycles);
                            IntPtr start;
                            if (NtQueryInformationThread(h, 9, out start, IntPtr.Size, IntPtr.Zero) == 0) th.Start = start.ToInt64();
                            IntPtr d;
                            if (GetThreadDescription(h, out d) >= 0 && d != IntPtr.Zero) {
                                th.Name = Marshal.PtrToStringUni(d);
                                LocalFree(d);
                            }
                            CloseHandle(h);
                        }
                        list.Add(th);
                    }
                    return list;
                }
                if (next == 0) return list;
                offset += next;
            }
        } finally {
            if (buf != IntPtr.Zero) Marshal.FreeHGlobal(buf);
        }
    }
}
'@

$proc = if ($Id) { Get-Process -Id $Id -ErrorAction Stop } else { Get-Process $Name -ErrorAction Stop | Select-Object -First 1 }
$procId = [uint32]$proc.Id
$before = @{}; foreach ($t in [IdlePerf]::Threads($procId)) { $before[$t.Id] = $t }
$cyclesBefore = [IdlePerf]::ProcessCycles($procId)
Start-Sleep -Seconds $Seconds
$after = [IdlePerf]::Threads($procId)
$cycles = [IdlePerf]::ProcessCycles($procId) - $cyclesBefore

"{0} pid {1} ({2}) over {3} s" -f $Name, $procId, $proc.Path, $Seconds
"process cycles: {0:N0} ({1:N0}/s)" -f $cycles, ($cycles / $Seconds)
# Name an unnamed thread by the module its start address is in (pool threads all start in ntdll).
$modules = $proc.Modules
function Start-Module([long]$address) {
    foreach ($m in $modules) {
        $base = $m.BaseAddress.ToInt64()
        if ($address -ge $base -and $address -lt $base + $m.ModuleMemorySize) {
            return '{0}+0x{1:X}' -f $m.ModuleName, ($address - $base)
        }
    }
    '-'
}
$rows = foreach ($t in $after) {
    $b = $before[$t.Id]
    $switches = if ($b) { $t.Switches - $b.Switches } else { $t.Switches }
    $cyc = if ($b) { $t.Cycles - $b.Cycles } else { $t.Cycles }
    [pscustomobject]@{
        Tid = $t.Id
        Name = if ($t.Name) { $t.Name } else { '-' }
        Start = Start-Module $t.Start
        Switches = $switches
        'Wakeups/s' = [math]::Round($switches / $Seconds, 2)
        Cycles = $cyc
    }
}
$total = ($rows | Measure-Object Switches -Sum).Sum
"total wakeups: {0} ({1:N2}/s)" -f $total, ($total / $Seconds)
$rows | Sort-Object Switches -Descending | Format-Table -AutoSize | Out-String -Width 200
