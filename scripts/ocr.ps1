param(
    [switch]$c,
    [switch]$i,
    [Parameter(ValueFromRemainingArguments=$true)]
    [string[]]$Query
)

if ($Query.Count -eq 0) {
    @'
ocr <query>
-c    dont use default dir
-i    do scan
'@
    exit 0
}

if (-not $c) {
    Set-Location (Join-Path $env:USERPROFILE 'Pictures') -ErrorAction SilentlyContinue
}

$tempFile = New-TemporaryFile
$title = $Query -join ' '

# Build ocrlocate arguments properly
$ocrlocateArgs = @()
if (-not $i) {
    $ocrlocateArgs += '-n'
}
$ocrlocateArgs += $Query

# Run ocrlocate and extract file paths (2nd column - cut -f2 equivalent)
ocrlocate @ocrlocateArgs | ForEach-Object {
    $fields = $_ -split "`t", 2
    if ($fields.Count -ge 2) {
        $fields[1]
    }
} | Set-Content $tempFile

if ((Get-Item $tempFile).Length -gt 0) {
    Start-Process 'C:\Program Files\IrfanView\i_view64.exe' -ArgumentList '/thumbs', "/filelist=`"$($tempFile.FullName)`"", "/title=`"$title`""
} else {
    'No results found'
    $tempFile | Remove-Item
}
