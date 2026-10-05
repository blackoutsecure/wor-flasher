$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$request = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__WOR_REQUEST__')) | ConvertFrom-Json
$result = @{ state = 'failed'; language = $request.language; changed = $false; stage = 'device-guard'; available = @() }
$directory = $null
$task = $null
$folder = $null
try {
    $product = (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion').ProductName
    if ($product -notmatch 'Windows\s+10\s+IoT\s+Core') { throw 'Not IoT Core' }
    $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    if ($sid -notmatch '-500$') { throw 'Not the administrator' }
    $defaultSid = $sid -replace '-500$', '-503'
    $result.stage = 'default-account-task'
    $name = 'WoR-IoT-Language-' + [Guid]::NewGuid().ToString('N')
    $directory = Join-Path $env:SystemDrive ('Data\ProgramData\WoR-Flasher\' + $name)
    $null = New-Item -ItemType Directory -Path $directory
    $acl = [Security.AccessControl.DirectorySecurity]::new()
    $acl.SetAccessRuleProtection($true, $false)
    foreach ($identity in @($sid, 'S-1-5-18', $defaultSid)) {
        $rights = if ($identity -eq $defaultSid) { [Security.AccessControl.FileSystemRights]::Modify } else { [Security.AccessControl.FileSystemRights]::FullControl }
        $rule = [Security.AccessControl.FileSystemAccessRule]::new(
            [Security.Principal.SecurityIdentifier]::new($identity), $rights,
            [Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit',
            [Security.AccessControl.PropagationFlags]::None, [Security.AccessControl.AccessControlType]::Allow)
        $acl.AddAccessRule($rule)
    }
    Set-Acl -LiteralPath $directory -AclObject $acl
    $resultPath = Join-Path $directory 'result.json'
    $payload = @{ language = $request.language; resultPath = $resultPath } | ConvertTo-Json -Compress
    $encoded = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($payload))
    $userScript = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__WOR_USER_SCRIPT__')).Replace('__WOR_LOCALE_USER_REQUEST__', $encoded)
    $scriptPath = Join-Path $directory 'language.ps1'
    [IO.File]::WriteAllText($scriptPath, $userScript, [Text.UTF8Encoding]::new($false))
    $service = New-Object -ComObject 'Schedule.Service'
    $service.Connect()
    $folder = $service.GetFolder('\')
    $definition = $service.NewTask(0)
    $definition.Principal.UserId = $defaultSid
    $definition.Principal.LogonType = 3
    $definition.Principal.RunLevel = 0
    $definition.Settings.Enabled = $true
    $definition.Settings.ExecutionTimeLimit = 'PT1M'
    $action = $definition.Actions.Create(0)
    $action.Path = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $action.Arguments = '-NoLogo -NoProfile -NonInteractive -File "' + $scriptPath + '"'
    $task = $folder.RegisterTaskDefinition($name, $definition, 6, $defaultSid, $null, 3, $null)
    $null = $task.Run($null)
    for ($attempt = 0; $attempt -lt 40; $attempt++) {
        if (Test-Path -LiteralPath $resultPath) {
            try {
                $observed = [IO.File]::ReadAllText($resultPath) | ConvertFrom-Json
                if ($observed.language -ine $request.language -or $observed.state -notin @('verified','pending-reboot','unsupported','failed')) { throw 'Invalid language result' }
                $result = $observed
                break
            } catch {
                if ($attempt -eq 39) { throw 'Incomplete language result' }
            }
        }
        Start-Sleep -Seconds 1
    }
} catch {
    $result.state = 'failed'
} finally {
    if ($task) {
        try { $task.Stop(0); $folder.DeleteTask($name, 0) } catch { $result.state = 'failed'; $result.stage = 'task-cleanup' }
    }
    if ($directory) {
        try { Remove-Item -LiteralPath $directory -Recurse -Force } catch { $result.state = 'failed'; $result.stage = 'task-cleanup' }
    }
}
[Console]::WriteLine('WOR_IOT_LOCALE_JSON:' + ($result | ConvertTo-Json -Compress))
