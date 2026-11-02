# Самотест модели через клиент: фразы и ожидаемый экран. «=» — набор как на экране.
param(
    [string]$Exe = "$PSScriptRoot\..\dist\AntiPunto\antipunto.exe",
    [string]$Device = "CPU",
    [string[]]$ExtraArgs = @()
)
$cases = [ordered]@{
    # набрано целиком в английской раскладке
    "привет, как дела? я в курсе, что ты вернулся " = "привет, как дела? я в курсе, что ты вернулся "
    "я сейчас создам проект на гитхаб, "             = "я сейчас создам проект на гитхаб, "
    # правильный текст не трогаем
    "=открой файл config в папке src "                = "открой файл config в папке src "
    "=я сейчас создам проект, потом напишу тебе как дела " = "я сейчас создам проект, потом напишу тебе как дела "
    "=I think it is a good idea to go "               = "I think it is a good idea to go "
    "=ну да, я в курсе, а ты как "                    = "ну да, я в курсе, а ты как "
    # английские слова в русской раскладке
    "=залей код на пшерги и открой ифтл "             = "залей код на github и открой bank "
    "=я создал репозиторий пшерги "                    = "я создал репозиторий github "
    "=прошивка для ыеь32 через фквгштщ "               = "прошивка для stm32 через arduino "
}
# орфография: исправить опечатку, не трогать редкие настоящие слова и жаргон
$spell = [ordered]@{
    "я создал преокт на гитхаб"            = "я создал проект на гитхаб"
    "вчера мы пошли в магазн и купили хлеб" = "вчера мы пошли в магазин и купили хлеб"
    "спасибо я пониаю"                      = "спасибо я понимаю"
    "спасибо, всё понятнл."                 = "спасибо, всё понятно."
    "я хочу удететь отсюда"                 = "я хочу улететь отсюда"
    "вентральным отделом и масти собак"     = "вентральным отделом и масти собак"
    "щас норм кароче"                       = "щас норм кароче"
    "i have a problme with this"            = "i have a problem with this"
}
$failed = 0
foreach ($c in $cases.GetEnumerator()) {
    $out = & $Exe --device $Device @ExtraArgs --selftest $c.Key --expect $c.Value 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) {
        $failed++
        Write-Host "ОШИБКА: $($c.Key)`n$out"
    } else {
        Write-Host "ok: $($c.Value)"
    }
}
foreach ($c in $spell.GetEnumerator()) {
    $out = & $Exe --device $Device @ExtraArgs --spelltest $c.Key --expect $c.Value 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) {
        $failed++
        Write-Host "ОШИБКА (орфография): $($c.Key)`n$out"
    } else {
        Write-Host "ok: $($c.Value)"
    }
}
if ($failed) { throw "самотест: $failed из $($cases.Count + $spell.Count) не прошли" }
