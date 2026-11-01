# Сборка дистрибутива: exe, OpenVINO (CPU и NPU), модель; zip, установщик, SHA256SUMS.
param(
    [string]$Version,      # без «v»; по умолчанию из client/Cargo.toml
    [string]$OvLibs,       # папка openvino/libs из pip-пакета openvino
    [switch]$NoBuild,
    [switch]$NoInstaller
)
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent

if (-not $Version) {
    $Version = (Select-String -Path "$root\client\Cargo.toml" -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
}
if (-not $OvLibs) {
    $OvLibs = python -c "import openvino, os; print(os.path.join(os.path.dirname(openvino.__file__), 'libs'))"
}
if (-not (Test-Path "$OvLibs\openvino_c.dll")) { throw "нет openvino_c.dll в $OvLibs" }

if (-not $NoBuild) {
    cargo build --release --manifest-path "$root\client\Cargo.toml"
    if ($LASTEXITCODE) { throw "cargo build" }
}

$dist = "$root\dist"
$app = "$dist\AntiPunto"
Remove-Item $dist -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$app\openvino", "$app\models" | Out-Null

Copy-Item "$root\client\target\release\antipunto.exe" $app
Copy-Item "$root\models\layout.xml", "$root\models\layout.bin" "$app\models"

# только то, что нужно для IR-модели на CPU и NPU
$dlls = @(
    "openvino.dll", "openvino_c.dll", "openvino_ir_frontend.dll",
    "openvino_intel_cpu_plugin.dll", "openvino_intel_npu_plugin.dll",
    "tbb12.dll", "tbbbind_2_5.dll", "tbbmalloc.dll", "tbbmalloc_proxy.dll"
)
foreach ($d in $dlls) { Copy-Item (Join-Path $OvLibs $d) "$app\openvino" }

$lic = Get-ChildItem (Join-Path $OvLibs "..\..") -Directory -Filter "openvino-*.dist-info" | Select-Object -First 1
if ($lic) { Copy-Item "$($lic.FullName)\licenses\*" "$app\openvino" -Recurse }
Copy-Item "$root\README.md" $app

$zip = "$dist\AntiPunto-$Version-portable.zip"
Compress-Archive "$app\*" $zip

if (-not $NoInstaller) {
    $iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $iscc) { throw "не найден Inno Setup 6 (ISCC.exe)" }
    & $iscc "/DAppVersion=$Version" "/O$dist" "$root\installer\antipunto.iss"
    if ($LASTEXITCODE) { throw "ISCC" }
}

$lines = Get-ChildItem $dist -File | Where-Object { $_.Name -match '\.(zip|exe)$' } | ForEach-Object {
    "{0}  {1}" -f (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower(), $_.Name
}
[IO.File]::WriteAllLines("$dist\SHA256SUMS.txt", [string[]]$lines)
Get-ChildItem $dist -File | Select-Object Name, @{n = 'MB'; e = { [math]::Round($_.Length / 1MB, 1) } }
