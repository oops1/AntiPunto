# Текст релиза: раздел версии из CHANGELOG.md плюс ссылки на установку.
param(
    [string]$Version = "",
    [string]$Out = ""
)
$root = Split-Path $PSScriptRoot -Parent
if (-not $Version) {
    $Version = (Select-String -Path "$root\client\Cargo.toml" -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
}
$lines = Get-Content "$root\CHANGELOG.md" -Encoding utf8
$start = -1
for ($i = 0; $i -lt $lines.Count; $i++) {
    if ($lines[$i] -match "^## $([regex]::Escape($Version))( |$)") { $start = $i + 1; break }
}
if ($start -lt 0) { throw "в CHANGELOG.md нет раздела ## $Version" }
$end = $lines.Count
for ($i = $start; $i -lt $lines.Count; $i++) {
    if ($lines[$i] -match '^## ') { $end = $i; break }
}
$body = ($lines[$start..($end - 1)] -join "`n").Trim()
if (-not $body) { throw "раздел $Version в CHANGELOG.md пустой" }
$notes = @"
$body

### Установка

Скачайте ``AntiPunto-$Version-setup.exe`` ниже — установка в профиль пользователя, права администратора не нужны.
Есть портативный zip. Контрольные суммы — в ``SHA256SUMS.txt``.
"@
if ($Out) { Set-Content -Path $Out -Value $notes -Encoding utf8 } else { $notes }
