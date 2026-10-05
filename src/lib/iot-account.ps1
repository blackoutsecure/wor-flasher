$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$request = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__WOR_REQUEST__')) | ConvertFrom-Json
$result = @{ state = 'failed'; passwordChanged = $false; usernameChanged = $false; username = ''; sid = ''; code = 0; stage = 'device-guard' }
try {
    $product = (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion').ProductName
    if ($product -notmatch 'Windows\s+10\s+IoT\s+Core') { throw 'Not IoT Core' }
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $sid = $identity.User.Value
    if ($sid -notmatch '-500$') { throw 'Not the built-in administrator' }
    $currentName = $identity.Name.Split('\')[-1]
    $result.username = $currentName
    $result.sid = $sid
    if ($request.operation -eq 'verify') {
        $result.stage = 'login-verification'
        if ($currentName -ine $request.username) { throw 'Unexpected account' }
        $result.state = 'verified'
    } else {
        $result.stage = 'native-preflight'
        $name = [Reflection.AssemblyName]::new('WorIotAccountNative')
        if ([AppDomain].GetMethod('DefineDynamicAssembly', [Type[]]@([Reflection.AssemblyName], [Reflection.Emit.AssemblyBuilderAccess]))) {
            $assembly = [AppDomain]::CurrentDomain.DefineDynamicAssembly($name, [Reflection.Emit.AssemblyBuilderAccess]::Run)
        } else {
            $assembly = [Reflection.Emit.AssemblyBuilder]::DefineDynamicAssembly($name, [Reflection.Emit.AssemblyBuilderAccess]::Run)
        }
        $module = $assembly.DefineDynamicModule('WorIotAccountNative')
        $type = $module.DefineType('WorIotAccountNative', [Reflection.TypeAttributes]'Public,Abstract,Sealed')
        $attributes = [Reflection.MethodAttributes]'Public,Static,PinvokeImpl'
        $convention = [Reflection.CallingConventions]::Standard
        $calling = [Runtime.InteropServices.CallingConvention]::Winapi
        $charset = [Runtime.InteropServices.CharSet]::Unicode
        $get = $type.DefinePInvokeMethod('NetUserGetInfo', 'netapi32.dll', $attributes, $convention, [UInt32],
            [Type[]]@([String], [String], [UInt32], [IntPtr].MakeByRefType()), $calling, $charset)
        $set = $type.DefinePInvokeMethod('NetUserSetInfo', 'netapi32.dll', $attributes, $convention, [UInt32],
            [Type[]]@([String], [String], [UInt32], [IntPtr], [UInt32].MakeByRefType()), $calling, $charset)
        $free = $type.DefinePInvokeMethod('NetApiBufferFree', 'netapi32.dll', $attributes, $convention, [UInt32],
            [Type[]]@([IntPtr]), $calling, $charset)
        foreach ($method in @($get, $set, $free)) { $method.SetImplementationFlags([Reflection.MethodImplAttributes]::PreserveSig) }
        $native = $type.CreateType()
        $buffer = [IntPtr]::Zero
        $code = $native::NetUserGetInfo($null, $currentName, 23, [ref]$buffer)
        if ($code -ne 0) { $result.code = $code; throw 'Account unavailable' }
        try {
            $sidOffset = [int]([Math]::Ceiling((3 * [IntPtr]::Size + 4) / [double][IntPtr]::Size) * [IntPtr]::Size)
            $localSid = [Security.Principal.SecurityIdentifier]::new([Runtime.InteropServices.Marshal]::ReadIntPtr($buffer, $sidOffset))
            if ($localSid.Value -ne $sid) { throw 'Not the local built-in administrator' }
        } finally { [void]$native::NetApiBufferFree($buffer) }
        if ($request.username -ine $currentName) {
            $buffer = [IntPtr]::Zero
            $code = $native::NetUserGetInfo($null, $request.username, 0, [ref]$buffer)
            if ($code -eq 0) { [void]$native::NetApiBufferFree($buffer); $result.code = 2224; throw 'Account exists' }
            if ($code -ne 2221) { $result.code = $code; throw 'Cannot check account name' }
        }
        foreach ($change in @(@{ level = 1003; value = $request.password }, @{ level = 0; value = $request.username })) {
            if ($change.level -eq 0 -and $request.username -ieq $currentName) { continue }
            $result.stage = if ($change.level -eq 1003) { 'password-update' } else { 'username-update' }
            $text = [Runtime.InteropServices.Marshal]::StringToHGlobalUni($change.value)
            $structure = [Runtime.InteropServices.Marshal]::AllocHGlobal([IntPtr]::Size)
            try {
                [Runtime.InteropServices.Marshal]::WriteIntPtr($structure, $text)
                $parameter = [UInt32]0
                $code = $native::NetUserSetInfo($null, $currentName, $change.level, $structure, [ref]$parameter)
                if ($code -ne 0) { $result.code = $code; throw 'Account update failed' }
                if ($change.level -eq 1003) { $result.passwordChanged = $true }
                else { $result.usernameChanged = $true; $result.username = $request.username }
            } finally {
                [Runtime.InteropServices.Marshal]::FreeHGlobal($structure)
                [Runtime.InteropServices.Marshal]::ZeroFreeGlobalAllocUnicode($text)
            }
        }
        $result.state = 'applied'
    }
} catch {
    $result.state = 'failed'
}
[Console]::WriteLine('WOR_IOT_ACCOUNT_JSON:' + ($result | ConvertTo-Json -Compress))
