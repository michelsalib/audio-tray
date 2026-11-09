<#
.SYNOPSIS
  Probes wireless-dongle HID interfaces for battery data (Logitech, NuPhy, Razer).

.DESCRIPTION
  Read-only prototype for audio-tray's battery feature. For every HID interface of the given vendors:
    1. dumps its collections (usage page/usage, report lengths, report ids, every usage it declares,
       flagging the standard battery usages);
    2. reads every feature report it declares;
    3. Logitech (046D): HID++ 2.0 on the receiver's vendor collections: ping devices 1..6, name, type,
       and battery features 0x1000 / 0x1001 / 0x1004 / 0x1F20;
    4. Razer (1532): the openrazer-style 90-byte "get battery" / "get charging" feature query;
    5. listens to every openable collection for -Listen seconds and logs each distinct report
       (dongles often push battery status on their own).
  Nothing is written to a device except HID++ "get" requests and the Razer "get" query.

  Windows PowerShell 5.1 or PowerShell 7:
    powershell -ExecutionPolicy Bypass -File hid-probe.ps1 [-Listen 20] [-Vid 046D,19F5,1532]
  Writes hid-probe-<COMPUTERNAME>.txt in the current folder.
#>
param(
    [int]$Listen = 20,
    [string[]]$Vid = @('046D', '19F5', '1532'),
    [string]$Out = (Join-Path (Get-Location) "hid-probe-$env:COMPUTERNAME.txt")
)

$src = @'
using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.Win32.SafeHandles;

public class HidIface {
    public int N;
    public string Path;
    public ushort Vid, Pid, Ver;
    public string Product = "", Manufacturer = "";
    public ushort UsagePage, Usage, InLen, OutLen, FeatLen;
    public List<string> Usages = new List<string>();
    public List<byte> FeatIds = new List<byte>();
    public bool Battery;
    public string Mi {
        get {
            Match m = Regex.Match(Path, "mi_([0-9a-f]+)", RegexOptions.IgnoreCase);
            return m.Success ? m.Groups[1].Value.ToUpper() : "-";
        }
    }
    public string Col {
        get {
            Match m = Regex.Match(Path, "col([0-9a-f]+)", RegexOptions.IgnoreCase);
            return m.Success ? m.Groups[1].Value.ToUpper() : "-";
        }
    }
}

// Overlapped reads with a timeout; a read that times out stays pending for the next call.
public class Reader : IDisposable {
    FileStream fs; Task<int> pending; byte[] buf; int len;
    public Reader(SafeFileHandle h, int len) { fs = new FileStream(h, FileAccess.Read, 1, true); this.len = len; }
    public byte[] Read(int ms) {
        try {
            if (pending == null) { buf = new byte[len]; pending = fs.ReadAsync(buf, 0, len); }
            if (!pending.Wait(ms)) return null;
            int n = pending.Result; pending = null;
            byte[] r = new byte[n]; Array.Copy(buf, r, n); return r;
        } catch { pending = null; return null; }
    }
    public void Dispose() { try { fs.Dispose(); } catch { } }
}

public static class HidProbe {
    [StructLayout(LayoutKind.Sequential)] struct SP_DEVICE_INTERFACE_DATA { public int cbSize; public Guid g; public int flags; public IntPtr reserved; }
    [StructLayout(LayoutKind.Sequential)] struct HIDD_ATTRIBUTES { public int Size; public ushort VendorID, ProductID, VersionNumber; }
    [StructLayout(LayoutKind.Sequential)] struct HIDP_CAPS {
        public ushort Usage, UsagePage, InputReportByteLength, OutputReportByteLength, FeatureReportByteLength;
        [MarshalAs(UnmanagedType.ByValArray, SizeConst = 17)] public ushort[] Reserved;
        public ushort NumberLinkCollectionNodes, NumberInputButtonCaps, NumberInputValueCaps, NumberInputDataIndices,
            NumberOutputButtonCaps, NumberOutputValueCaps, NumberOutputDataIndices,
            NumberFeatureButtonCaps, NumberFeatureValueCaps, NumberFeatureDataIndices;
    }

    [DllImport("hid.dll")] static extern void HidD_GetHidGuid(out Guid g);
    [DllImport("hid.dll")] static extern bool HidD_GetAttributes(SafeFileHandle h, ref HIDD_ATTRIBUTES a);
    [DllImport("hid.dll")] static extern bool HidD_GetProductString(SafeFileHandle h, byte[] b, int len);
    [DllImport("hid.dll")] static extern bool HidD_GetManufacturerString(SafeFileHandle h, byte[] b, int len);
    [DllImport("hid.dll")] static extern bool HidD_GetPreparsedData(SafeFileHandle h, out IntPtr pp);
    [DllImport("hid.dll")] static extern bool HidD_FreePreparsedData(IntPtr pp);
    [DllImport("hid.dll")] static extern int HidP_GetCaps(IntPtr pp, out HIDP_CAPS c);
    [DllImport("hid.dll")] static extern int HidP_GetValueCaps(int type, IntPtr caps, ref ushort len, IntPtr pp);
    [DllImport("hid.dll")] static extern int HidP_GetButtonCaps(int type, IntPtr caps, ref ushort len, IntPtr pp);
    [DllImport("hid.dll", SetLastError = true)] static extern bool HidD_GetFeature(SafeFileHandle h, byte[] b, int len);
    [DllImport("hid.dll", SetLastError = true)] static extern bool HidD_SetFeature(SafeFileHandle h, byte[] b, int len);
    [DllImport("hid.dll", SetLastError = true)] static extern bool HidD_SetOutputReport(SafeFileHandle h, byte[] b, int len);
    [DllImport("setupapi.dll", CharSet = CharSet.Unicode)] static extern IntPtr SetupDiGetClassDevsW(ref Guid g, IntPtr e, IntPtr hwnd, int flags);
    [DllImport("setupapi.dll")] static extern bool SetupDiEnumDeviceInterfaces(IntPtr set, IntPtr devinfo, ref Guid g, int idx, ref SP_DEVICE_INTERFACE_DATA d);
    [DllImport("setupapi.dll", CharSet = CharSet.Unicode)] static extern bool SetupDiGetDeviceInterfaceDetailW(IntPtr set, ref SP_DEVICE_INTERFACE_DATA d, IntPtr detail, int size, out int req, IntPtr devinfo);
    [DllImport("setupapi.dll")] static extern bool SetupDiDestroyDeviceInfoList(IntPtr set);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern SafeFileHandle CreateFileW(string p, uint access, uint share, IntPtr sa, uint disp, uint flags, IntPtr t);

    const uint RW = 0xC0000000, WRITE = 0x40000000, READ = 0x80000000, SHARE = 3, OPEN = 3, OVERLAPPED = 0x40000000;
    const int HIDP_OK = 0x00110000;

    static StringBuilder log = new StringBuilder();
    static void L(string s) { log.AppendLine(s); }
    static string Hex(byte[] b) { return b == null ? "(none)" : BitConverter.ToString(b).Replace("-", " "); }
    static string Hex(byte[] b, int n) {
        if (b == null) return "(none)";
        byte[] c = new byte[Math.Min(n, b.Length)]; Array.Copy(b, c, c.Length); return Hex(c);
    }

    static SafeFileHandle Open(string path, uint access, uint flags) {
        SafeFileHandle h = CreateFileW(path, access, SHARE, IntPtr.Zero, OPEN, flags, IntPtr.Zero);
        if (h.IsInvalid) { h.Dispose(); return null; }
        return h;
    }

    // ---------- enumeration ----------

    public static List<HidIface> Enumerate(string[] vids) {
        List<HidIface> list = new List<HidIface>();
        Guid g; HidD_GetHidGuid(out g);
        IntPtr set = SetupDiGetClassDevsW(ref g, IntPtr.Zero, IntPtr.Zero, 0x12);
        for (int i = 0; ; i++) {
            SP_DEVICE_INTERFACE_DATA d = new SP_DEVICE_INTERFACE_DATA(); d.cbSize = Marshal.SizeOf(d);
            if (!SetupDiEnumDeviceInterfaces(set, IntPtr.Zero, ref g, i, ref d)) break;
            int req; SetupDiGetDeviceInterfaceDetailW(set, ref d, IntPtr.Zero, 0, out req, IntPtr.Zero);
            IntPtr buf = Marshal.AllocHGlobal(req);
            Marshal.WriteInt32(buf, IntPtr.Size == 8 ? 8 : 6);
            string path = null;
            if (SetupDiGetDeviceInterfaceDetailW(set, ref d, buf, req, out req, IntPtr.Zero)) path = Marshal.PtrToStringUni(IntPtr.Add(buf, 4));
            Marshal.FreeHGlobal(buf);
            if (path == null) continue;
            Match m = Regex.Match(path, "vid[_&](?:0[12](?=[0-9a-f]{4}[_&#]))?([0-9a-f]{4})", RegexOptions.IgnoreCase);
            if (!m.Success || Array.IndexOf(vids, m.Groups[1].Value.ToUpper()) < 0) continue;
            HidIface f = Describe(path);
            if (f != null) { f.N = list.Count; list.Add(f); }
        }
        SetupDiDestroyDeviceInfoList(set);
        return list;
    }

    static HidIface Describe(string path) {
        using (SafeFileHandle h = Open(path, 0, 0)) {
            if (h == null) return null;
            HidIface f = new HidIface(); f.Path = path;
            HIDD_ATTRIBUTES a = new HIDD_ATTRIBUTES(); a.Size = Marshal.SizeOf(a);
            if (HidD_GetAttributes(h, ref a)) { f.Vid = a.VendorID; f.Pid = a.ProductID; f.Ver = a.VersionNumber; }
            byte[] s = new byte[512];
            if (HidD_GetProductString(h, s, s.Length)) f.Product = Encoding.Unicode.GetString(s).TrimEnd('\0');
            s = new byte[512];
            if (HidD_GetManufacturerString(h, s, s.Length)) f.Manufacturer = Encoding.Unicode.GetString(s).TrimEnd('\0');
            IntPtr pp;
            if (!HidD_GetPreparsedData(h, out pp)) return f;
            HIDP_CAPS c;
            if (HidP_GetCaps(pp, out c) == HIDP_OK) {
                f.UsagePage = c.UsagePage; f.Usage = c.Usage;
                f.InLen = c.InputReportByteLength; f.OutLen = c.OutputReportByteLength; f.FeatLen = c.FeatureReportByteLength;
                string[] names = { "In", "Out", "Feat" };
                ushort[] nb = { c.NumberInputButtonCaps, c.NumberOutputButtonCaps, c.NumberFeatureButtonCaps };
                ushort[] nv = { c.NumberInputValueCaps, c.NumberOutputValueCaps, c.NumberFeatureValueCaps };
                for (int t = 0; t < 3; t++) {
                    Caps(f, pp, t, nb[t], false, names[t]);
                    Caps(f, pp, t, nv[t], true, names[t]);
                }
            }
            HidD_FreePreparsedData(pp);
            return f;
        }
    }

    // HIDP_BUTTON_CAPS / HIDP_VALUE_CAPS are both 72 bytes; UsagePage@0, ReportID@2, IsRange@12, Usage(Min)@56, UsageMax@58.
    static void Caps(HidIface f, IntPtr pp, int type, ushort count, bool value, string tname) {
        if (count == 0) return;
        ushort n = count; IntPtr mem = Marshal.AllocHGlobal(72 * n);
        int st = value ? HidP_GetValueCaps(type, mem, ref n, pp) : HidP_GetButtonCaps(type, mem, ref n, pp);
        if (st == HIDP_OK) {
            for (int i = 0; i < n; i++) {
                IntPtr p = IntPtr.Add(mem, 72 * i);
                ushort page = (ushort)Marshal.ReadInt16(p, 0);
                byte rid = Marshal.ReadByte(p, 2);
                bool range = Marshal.ReadByte(p, 12) != 0;
                ushort u0 = (ushort)Marshal.ReadInt16(p, 56), u1 = (ushort)Marshal.ReadInt16(p, 58);
                string u = range ? string.Format("{0:X2}-{1:X2}", u0, u1) : u0.ToString("X2");
                string extra = "";
                if (value) {
                    ushort bits = (ushort)Marshal.ReadInt16(p, 18), cnt = (ushort)Marshal.ReadInt16(p, 20);
                    int lmin = Marshal.ReadInt32(p, 40), lmax = Marshal.ReadInt32(p, 44);
                    extra = string.Format(" {0}x{1}b [{2}..{3}]", cnt, bits, lmin, lmax);
                }
                bool batt = (page == 0x06 && (u0 == 0x20 || (range && u0 <= 0x20 && u1 >= 0x20))) || page == 0x84 || page == 0x85;
                if (batt) f.Battery = true;
                f.Usages.Add(string.Format("{0} rid={1:X2} {2:X4}:{3}{4}{5}{6}", tname, rid, page, u, value ? " val" : " btn", extra, batt ? "  <== BATTERY" : ""));
                if (type == 2 && !f.FeatIds.Contains(rid)) f.FeatIds.Add(rid);
            }
        }
        Marshal.FreeHGlobal(mem);
    }

    // ---------- feature reports ----------

    static void ReadFeatures(HidIface f) {
        if (f.FeatLen == 0) return;
        using (SafeFileHandle h = Open(f.Path, 0, 0)) {
            if (h == null) return;
            foreach (byte rid in f.FeatIds) {
                byte[] b = new byte[f.FeatLen]; b[0] = rid;
                bool ok = HidD_GetFeature(h, b, b.Length);
                L(string.Format("    GetFeature rid={0:X2}: {1}", rid, ok ? Hex(b) : "failed (" + Marshal.GetLastWin32Error() + ")"));
            }
        }
    }

    // ---------- Logitech HID++ 2.0 ----------

    class Hpp {
        public Reader Short, Long; public FileStream W; public SafeFileHandle WH; public int ShortLen;
    }

    static byte[] Req(Hpp c, byte dev, byte feat, byte fn, params byte[] p) {
        byte fs = (byte)((fn << 4) | 0x0A);
        byte[] r = new byte[c.ShortLen]; r[0] = 0x10; r[1] = dev; r[2] = feat; r[3] = fs;
        for (int i = 0; i < p.Length && 4 + i < r.Length; i++) r[4 + i] = p[i];
        while (c.Short.Read(1) != null) { }
        if (c.Long != null) while (c.Long.Read(1) != null) { }
        try { c.W.Write(r, 0, r.Length); c.W.Flush(); }
        catch { if (!HidD_SetOutputReport(c.WH, r, r.Length)) return null; }
        DateTime end = DateTime.Now.AddMilliseconds(800);
        while (DateTime.Now < end) {
            byte[] x = (c.Long != null ? c.Long.Read(15) : null) ?? c.Short.Read(15);
            if (x == null || x.Length < 5 || x[1] != dev) continue;
            if (x[2] == feat && x[3] == fs) return x;
            if ((x[2] == 0x8F || x[2] == 0xFF) && x[3] == feat && x[4] == fs) return x;
        }
        return null;
    }
    static bool IsErr(byte[] x) { return x != null && (x[2] == 0x8F || x[2] == 0xFF); }

    static byte Feature(Hpp c, byte dev, ushort id) {
        byte[] x = Req(c, dev, 0, 0, (byte)(id >> 8), (byte)id);
        return (x == null || IsErr(x)) ? (byte)0 : x[4];
    }

    // Solaar's discharge curve for feature 0x1001 (mV -> %).
    static readonly int[,] Curve = { {4186,100},{4067,90},{3989,80},{3922,70},{3859,60},{3811,50},{3778,40},{3751,30},{3717,20},{3671,10},{3646,5},{3579,2},{3500,0} };
    static int VoltPct(int mv) {
        if (mv >= Curve[0, 0]) return 100;
        for (int i = 1; i < Curve.GetLength(0); i++)
            if (mv >= Curve[i, 0]) {
                double t = (double)(mv - Curve[i, 0]) / (Curve[i - 1, 0] - Curve[i, 0]);
                return (int)Math.Round(Curve[i, 1] + t * (Curve[i - 1, 1] - Curve[i, 1]));
            }
        return 0;
    }

    static void Logitech(List<HidIface> all) {
        Dictionary<string, List<HidIface>> groups = new Dictionary<string, List<HidIface>>();
        foreach (HidIface f in all) {
            if (f.Vid != 0x046D || f.UsagePage != 0xFF00) continue;
            string k = f.Pid.ToString("X4") + "/" + f.Mi;
            if (!groups.ContainsKey(k)) groups[k] = new List<HidIface>();
            groups[k].Add(f);
        }
        foreach (KeyValuePair<string, List<HidIface>> kv in groups) {
            HidIface sf = null, lf = null;
            foreach (HidIface f in kv.Value) { if (f.Usage == 1) sf = f; if (f.Usage == 2) lf = f; }
            if (sf == null) continue;
            L(""); L("== Logitech HID++ on " + kv.Key + " (" + sf.Product + ")");
            Hpp c = new Hpp(); c.ShortLen = sf.OutLen > 0 ? sf.OutLen : 7;
            SafeFileHandle hs = Open(sf.Path, RW, OVERLAPPED), hl = lf != null ? Open(lf.Path, RW, OVERLAPPED) : null;
            c.WH = Open(sf.Path, WRITE, 0);
            if (hs == null || c.WH == null) { L("  cannot open the HID++ collections (error " + Marshal.GetLastWin32Error() + ")"); continue; }
            c.Short = new Reader(hs, sf.InLen); c.Long = hl != null ? new Reader(hl, lf.InLen) : null;
            c.W = new FileStream(c.WH, FileAccess.Write, 1, false);
            try {
                for (byte dev = 1; dev <= 6; dev++) {
                    byte[] ping = Req(c, dev, 0, 1, 0, 0, 0x5A);
                    if (ping == null) { L(string.Format("  dev {0}: no answer", dev)); continue; }
                    if (IsErr(ping)) { L(string.Format("  dev {0}: error {1}", dev, Hex(ping, 7))); continue; }
                    L(string.Format("  dev {0}: HID++ {1}.{2}", dev, ping[4], ping[5]));
                    byte nameIdx = Feature(c, dev, 0x0005);
                    if (nameIdx != 0) {
                        byte[] cnt = Req(c, dev, nameIdx, 0);
                        int n = cnt != null && !IsErr(cnt) ? cnt[4] : 0;
                        StringBuilder name = new StringBuilder();
                        while (name.Length < n) {
                            byte[] x = Req(c, dev, nameIdx, 1, (byte)name.Length);
                            if (x == null || IsErr(x)) break;
                            int before = name.Length;
                            for (int i = 4; i < x.Length && name.Length < n; i++) name.Append((char)x[i]);
                            if (name.Length == before) break;
                        }
                        byte[] ty = Req(c, dev, nameIdx, 2);
                        L(string.Format("    name: \"{0}\"  type: {1}", name.ToString().TrimEnd('\0'), ty != null && !IsErr(ty) ? ty[4].ToString() + " (0 kb, 3 mouse, 8 headset)" : "?"));
                    }
                    ushort[] ids = { 0x1000, 0x1001, 0x1004, 0x1F20 };
                    foreach (ushort id in ids) {
                        byte idx = Feature(c, dev, id);
                        if (idx == 0) { L(string.Format("    0x{0:X4}: absent", id)); continue; }
                        byte[] x;
                        if (id == 0x1000) {
                            x = Req(c, dev, idx, 0);
                            L(string.Format("    0x1000 BATTERY_STATUS idx {0}: {1}", idx, Hex(x, 8)));
                            if (x != null && !IsErr(x)) L(string.Format("      => {0}% (next {1}%), status {2}", x[4], x[5], x[6]));
                        } else if (id == 0x1001) {
                            x = Req(c, dev, idx, 0);
                            L(string.Format("    0x1001 BATTERY_VOLTAGE idx {0}: {1}", idx, Hex(x, 8)));
                            if (x != null && !IsErr(x)) { int mv = (x[4] << 8) | x[5]; L(string.Format("      => {0} mV ~ {1}%, flags 0x{2:X2}", mv, VoltPct(mv), x[6])); }
                        } else if (id == 0x1004) {
                            byte[] cap = Req(c, dev, idx, 0);
                            x = Req(c, dev, idx, 1);
                            L(string.Format("    0x1004 UNIFIED_BATTERY idx {0}: caps {1} / status {2}", idx, Hex(cap, 8), Hex(x, 8)));
                            if (x != null && !IsErr(x)) L(string.Format("      => {0}%, level bits 0x{1:X2}, charging {2}, external {3}", x[4], x[5], x[6], x[7]));
                        } else {
                            x = Req(c, dev, idx, 0);
                            L(string.Format("    0x1F20 ADC_MEASUREMENT idx {0}: {1}", idx, Hex(x, 8)));
                            if (x != null && !IsErr(x)) { int mv = (x[4] << 8) | x[5]; L(string.Format("      => {0} mV ~ {1}%, flags 0x{2:X2}", mv, VoltPct(mv), x[6])); }
                        }
                    }
                }
            } finally {
                c.Short.Dispose(); if (c.Long != null) c.Long.Dispose(); c.W.Dispose();
            }
        }
    }

    // ---------- Razer ----------

    // BlackShark V2 Pro (2020) / Nari receivers: feature report FF, as HaloBattery's
    // providers/blackshark2020.py does it. Reply: [11] power state (09 charging, 06 full, 01-03 on battery),
    // [12..13] mV big-endian, [14] level %. A reply starting FF 01 00 is stale (headset off).
    static void RazerFeatureFF(List<HidIface> all) {
        byte[] prefix = { 0xFF, 0x0F, 0x05, 0xFE, 0x12, 0x04, 0x1F, 0x08, 0x05 };
        foreach (HidIface f in all) {
            if (f.Vid != 0x1532 || f.UsagePage != 0xFF00 || f.Usage != 1 || f.FeatLen != 64) continue;
            L(""); L(string.Format("== Razer battery (feature FF) on #{0} {1:X4}", f.N, f.Pid));
            using (SafeFileHandle h = Open(f.Path, 0, 0)) {
                if (h == null) continue;
                byte[] req = new byte[64];
                byte[] head = { 0xFF, 0x0A, 0x00, 0xFD, 0x04, 0x12, 0xF1, 0x02, 0x05 };
                Array.Copy(head, req, head.Length);
                if (!HidD_SetFeature(h, req, req.Length)) { L("  SetFeature failed (" + Marshal.GetLastWin32Error() + ")"); continue; }
                for (int attempt = 0; attempt < 6; attempt++) {
                    Thread.Sleep(100);
                    byte[] r = new byte[64]; r[0] = 0xFF;
                    if (!HidD_GetFeature(h, r, r.Length)) { L("  GetFeature failed (" + Marshal.GetLastWin32Error() + ")"); break; }
                    L("  reply: " + Hex(r, 20));
                    bool valid = true;
                    for (int i = 0; i < prefix.Length; i++) if (r[i] != prefix[i]) valid = false;
                    if (!valid) continue;
                    int mv = (r[12] << 8) | r[13];
                    string state = r[11] == 0x09 ? "charging" : r[11] == 0x06 ? "charged (plugged)" : (r[11] >= 1 && r[11] <= 3) ? "on battery" : "unknown";
                    L(string.Format("  => level {0}%, {1} mV, state 0x{2:X2} ({3})", r[14], mv, r[11], state));
                    break;
                }
            }
        }
    }

    static void Razer(List<HidIface> all) {
        RazerFeatureFF(all);
        foreach (HidIface f in all) {
            if (f.Vid != 0x1532 || f.FeatLen != 91) continue;
            L(""); L(string.Format("== Razer feature query on #{0} {1:X4}:{2:X4} mi {3}", f.N, f.UsagePage, f.Usage, f.Mi));
            using (SafeFileHandle h = Open(f.Path, 0, 0)) {
                if (h == null) continue;
                byte[] tids = { 0xFF, 0x1F, 0x3F, 0x9F, 0x08, 0x60 };
                byte[,] cmds = { { 0x07, 0x80 }, { 0x07, 0x84 } };
                foreach (byte tid in tids)
                    for (int k = 0; k < 2; k++) {
                        byte[] b = new byte[91];
                        b[2] = tid; b[6] = 0x02; b[7] = cmds[k, 0]; b[8] = cmds[k, 1];
                        byte crc = 0; for (int i = 3; i < 89; i++) crc ^= b[i]; b[89] = crc;
                        if (!HidD_SetFeature(h, b, b.Length)) { L(string.Format("  tid {0:X2} cmd {1:X2}{2:X2}: SetFeature failed ({3})", tid, cmds[k, 0], cmds[k, 1], Marshal.GetLastWin32Error())); continue; }
                        Thread.Sleep(80);
                        byte[] r = new byte[91];
                        bool ok = HidD_GetFeature(h, r, r.Length);
                        L(string.Format("  tid {0:X2} cmd {1:X2}{2:X2}: {3}", tid, cmds[k, 0], cmds[k, 1], ok ? Hex(r, 16) : "GetFeature failed"));
                        if (ok && r[1] == 0x02) L(string.Format("    => status OK, value {0} ({1}% if battery)", r[10], r[10] * 100 / 255));
                    }
            }
        }
    }

    // ---------- passive listen ----------

    static void Listen(List<HidIface> all, int secs) {
        L(""); L("== Listening " + secs + " s for unsolicited input reports");
        List<Reader> rs = new List<Reader>(); List<HidIface> fs = new List<HidIface>();
        foreach (HidIface f in all) {
            if (f.InLen == 0) continue;
            if (f.UsagePage == 1 && (f.Usage == 2 || f.Usage == 6)) continue; // mouse/keyboard: Windows holds them
            SafeFileHandle h = Open(f.Path, RW, OVERLAPPED) ?? Open(f.Path, READ, OVERLAPPED);
            if (h == null) { L(string.Format("  #{0}: cannot open ({1})", f.N, Marshal.GetLastWin32Error())); continue; }
            rs.Add(new Reader(h, f.InLen)); fs.Add(f);
        }
        Dictionary<string, int> seen = new Dictionary<string, int>();
        DateTime start = DateTime.Now, end = start.AddSeconds(secs);
        while (DateTime.Now < end) {
            for (int i = 0; i < rs.Count; i++) {
                byte[] x = rs[i].Read(5);
                if (x == null) continue;
                string k = fs[i].N + " " + Hex(x);
                if (seen.ContainsKey(k)) { seen[k]++; continue; }
                seen[k] = 1;
                L(string.Format("  [{0,5:0.0}s] #{1} {2:X4}:{3:X4} {4}", (DateTime.Now - start).TotalSeconds, fs[i].N, fs[i].UsagePage, fs[i].Usage, Hex(x)));
            }
        }
        foreach (KeyValuePair<string, int> kv in seen) if (kv.Value > 1) L("  repeated x" + kv.Value + ": #" + kv.Key);
        foreach (Reader r in rs) r.Dispose();
    }

    // ---------- entry ----------

    public static string Run(string[] vids, int listen) {
        for (int i = 0; i < vids.Length; i++) vids[i] = vids[i].ToUpper();
        List<HidIface> all = Enumerate(vids);
        L("== " + all.Count + " HID collection(s)");
        foreach (HidIface f in all) {
            L(""); L(string.Format("#{0} {1:X4}:{2:X4} v{3:X4} mi {4} col {5}  \"{6}\" / \"{7}\"", f.N, f.Vid, f.Pid, f.Ver, f.Mi, f.Col, f.Manufacturer, f.Product));
            L(string.Format("    page {0:X4} usage {1:X4}  in {2} out {3} feat {4}{5}", f.UsagePage, f.Usage, f.InLen, f.OutLen, f.FeatLen, f.Battery ? "  ** declares a battery usage **" : ""));
            L("    " + f.Path);
            foreach (string u in f.Usages) L("    " + u);
            ReadFeatures(f);
        }
        try { Logitech(all); } catch (Exception e) { L("Logitech probe failed: " + e); }
        try { Razer(all); } catch (Exception e) { L("Razer probe failed: " + e); }
        if (listen > 0) try { Listen(all, listen); } catch (Exception e) { L("Listen failed: " + e); }
        return log.ToString();
    }
}
'@

Add-Type -TypeDefinition $src -Language CSharp

$header = @"
hid-probe on $env:COMPUTERNAME, $(Get-Date -Format s), PowerShell $($PSVersionTable.PSVersion)
Vendors: $($Vid -join ', ')

== PnP devices of these vendors (present)
"@
$pattern = ($Vid | ForEach-Object { "VID_$_|VID&0[12]$_" }) -join '|'
$pnp = Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -match $pattern -and $_.InstanceId -notmatch '^HID\\' } |
    Sort-Object InstanceId | ForEach-Object { "  {0,-40} {1}" -f $_.FriendlyName, $_.InstanceId }

Write-Host "Probing... (listening $Listen s at the end: use the devices, plug/unplug a charging cable)"
$body = [HidProbe]::Run($Vid, $Listen)
$text = $header + "`n" + ($pnp -join "`n") + "`n`n" + $body
$text | Set-Content -Path $Out -Encoding UTF8
Write-Host $text
Write-Host "`nSaved to $Out"
