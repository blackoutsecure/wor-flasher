$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$request = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__WOR_LOCALE_USER_REQUEST__')) | ConvertFrom-Json
$result = @{ state = 'failed'; language = $request.language; changed = $false; stage = 'default-account-guard'; available = @() }
try {
    $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    if ($sid -notmatch '-503$') { throw 'Not DefaultAccount' }
    $tool = Join-Path $env:SystemRoot 'System32\IoTSettings.exe'
    if (-not (Test-Path -LiteralPath $tool)) { throw 'IoTSettings unavailable' }
    $result.stage = 'language-support'
    $listed = (& $tool list uilanguage 2>&1 | Out-String)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot list installed languages' }
    $languages = @([regex]::Matches($listed, '\b[A-Za-z]{2,3}(?:-[A-Za-z0-9]{2,8}){1,2}\b') | ForEach-Object { $_.Value } | Select-Object -Unique)
    $result.available = $languages
    if ($languages -inotcontains $request.language) { $result.state = 'unsupported'; throw 'Language not installed' }
    $current = (& $tool get uilanguage 2>&1 | Out-String)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read current language' }
    $matches = @([regex]::Matches($current, '\b[A-Za-z]{2,3}(?:-[A-Za-z0-9]{2,8}){1,2}\b') | ForEach-Object { $_.Value })
    if ($matches -icontains $request.language) {
        $result.state = 'verified'
    } else {
        $result.stage = 'language-update'
        $null = & $tool set uilanguage $request.language 2>&1
        if ($LASTEXITCODE -ne 0) { throw 'Cannot apply selected language' }
        $result.changed = $true
        $result.stage = 'language-verification'
        $current = (& $tool get uilanguage 2>&1 | Out-String)
        if ($LASTEXITCODE -ne 0) { throw 'Cannot verify selected language' }
        $matches = @([regex]::Matches($current, '\b[A-Za-z]{2,3}(?:-[A-Za-z0-9]{2,8}){1,2}\b') | ForEach-Object { $_.Value })
        $result.state = if ($matches -icontains $request.language) { 'verified' } else { 'pending-reboot' }
    }
} catch {
    if ($result.state -ne 'unsupported') { $result.state = 'failed' }
}
[IO.File]::WriteAllText($request.resultPath, ($result | ConvertTo-Json -Compress), [Text.UTF8Encoding]::new($false))
