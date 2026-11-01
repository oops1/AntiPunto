# Автотест: своё окно с полем ввода, набор скан-кодами, результат в файл.
param([string]$Out = "$env:TEMP\antipunto_autotest.txt", [int]$KeyMs = 45)

Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class KB {
  [StructLayout(LayoutKind.Sequential)] struct KI { public ushort vk; public ushort scan; public uint flags; public uint time; public IntPtr extra; }
  [StructLayout(LayoutKind.Explicit, Size = 40)] struct IN { [FieldOffset(0)] public uint type; [FieldOffset(8)] public KI ki; }
  [DllImport("user32.dll")] static extern uint SendInput(uint n, IN[] i, int size);
  public static void Scan(ushort sc, bool up) {
    var i = new IN[1]; i[0].type = 1; i[0].ki.scan = sc; i[0].ki.flags = 0x0008u | (up ? 2u : 0u);
    SendInput(1, i, Marshal.SizeOf(typeof(IN)));
  }
}
"@

$rows = @(
  @("``1234567890-=", "~!@#$%^&*()_+", @(0x29,0x02,0x03,0x04,0x05,0x06,0x07,0x08,0x09,0x0A,0x0B,0x0C,0x0D)),
  @("qwertyuiop[]\", "QWERTYUIOP{}|", @(0x10,0x11,0x12,0x13,0x14,0x15,0x16,0x17,0x18,0x19,0x1A,0x1B,0x2B)),
  @("asdfghjkl;'", "ASDFGHJKL:`"", @(0x1E,0x1F,0x20,0x21,0x22,0x23,0x24,0x25,0x26,0x27,0x28)),
  @("zxcvbnm,./", "ZXCVBNM<>?", @(0x2C,0x2D,0x2E,0x2F,0x30,0x31,0x32,0x33,0x34,0x35))
)
$map = New-Object System.Collections.Hashtable ([StringComparer]::Ordinal)
foreach ($r in $rows) {
  for ($i = 0; $i -lt $r[2].Count; $i++) {
    $map[[string]$r[0][$i]] = @($r[2][$i], $false)
    $map[[string]$r[1][$i]] = @($r[2][$i], $true)
  }
}
$map[" "] = @(0x39, $false)

# физические клавиши, как на английской раскладке
$steps = @("ghbdtn rfr ltkf ", "z d rehct ", "hello world ")
$expect = "привет как дела я в курсе hello world "

$form = New-Object Windows.Forms.Form
$form.Text = "AntiPunto autotest"; $form.Width = 720; $form.Height = 200; $form.TopMost = $true
$box = New-Object Windows.Forms.TextBox
$box.Multiline = $true; $box.Dock = "Fill"; $box.Font = New-Object Drawing.Font("Segoe UI", 14)
$form.Controls.Add($box)

$script:queue = New-Object System.Collections.Queue
$timer = New-Object Windows.Forms.Timer
$timer.Interval = $KeyMs
$form.Add_Shown({
  $form.Activate(); $box.Focus()
  [Windows.Forms.InputLanguage]::CurrentInputLanguage = [Windows.Forms.InputLanguage]::FromCulture([Globalization.CultureInfo]::GetCultureInfo("en-US"))
  $script:queue.Enqueue("wait:600")
  foreach ($s in $steps) { foreach ($c in $s.ToCharArray()) { $script:queue.Enqueue([string]$c) }; $script:queue.Enqueue("wait:500") }
  $script:queue.Enqueue("done")
  $timer.Start()
})
$script:waitUntil = [DateTime]::MinValue
$timer.Add_Tick({
  if ([DateTime]::Now -lt $script:waitUntil) { return }
  if (-not $form.ContainsFocus) { $timer.Stop(); $box.AppendText("`r`n[ПРЕРВАНО: окно потеряло фокус]"); $script:queue.Clear(); $script:queue.Enqueue("done"); $timer.Start(); }
  $item = $script:queue.Dequeue()
  if ($item -like "wait:*") { $script:waitUntil = [DateTime]::Now.AddMilliseconds([int]$item.Substring(5)); return }
  if ($item -eq "done") {
    $timer.Stop()
    $got = $box.Text
    $ok = $got -eq $expect
    "ожидалось: [$expect]`nполучено:  [$got]`nсовпало: $ok" | Out-File -FilePath $Out -Encoding utf8
    $form.Close(); return
  }
  $k = $map[$item]
  if ($k[1]) { [KB]::Scan(0x2A, $false) }
  [KB]::Scan([uint16]$k[0], $false); [KB]::Scan([uint16]$k[0], $true)
  if ($k[1]) { [KB]::Scan(0x2A, $true) }
})
[Windows.Forms.Application]::Run($form)
