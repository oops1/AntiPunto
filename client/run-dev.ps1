# Запуск для разработки: OpenVINO из venv, модель с диска G.
param([switch]$Release, [string]$Device = "NPU,CPU", [double]$Threshold = 0.6)
$exe = if ($Release) { "$PSScriptRoot\target\release\antipunto.exe" } else { "$PSScriptRoot\target\debug\antipunto.exe" }
& $exe --ov "$PSScriptRoot\..\train\venv\Lib\site-packages\openvino\libs" `
       --model "$PSScriptRoot\..\models\layout.xml" `
       --device $Device --threshold $Threshold --debug --no-update-check
