param([Parameter(Mandatory=$true)][string]$Path)
$ErrorActionPreference='Stop'
$forbidden=@('zhizi.ihongui','--set-consent')
$item=Get-Item -LiteralPath $Path
$files=if($item.PSIsContainer){Get-ChildItem -LiteralPath $item.FullName -Recurse -File -Force}else{@($item)}
foreach($file in $files){
 if($file.Name -ieq 'zhizi-upload.exe' -or $file.Name -ieq 'consent.json'){throw "Public package check failed: forbidden file $($file.Name)"}
 # Latin1 preserves ASCII strings even in binary payloads; UTF16 covers Windows text lists.
 $bytes=[IO.File]::ReadAllBytes($file.FullName)
 $contents=@($file.FullName,[Text.Encoding]::GetEncoding(28591).GetString($bytes),[Text.Encoding]::Unicode.GetString($bytes),[Text.Encoding]::BigEndianUnicode.GetString($bytes))
 # Exempt only the project-term row; any other occurrence must still be rejected.
 foreach($content in $contents){
  $withoutTerms=[regex]::Replace($content,'ZHIZI_TEST_INVITE\tzhizi_test_invite(?=\t|\r|\n|$)','',[Text.RegularExpressions.RegexOptions]::IgnoreCase)
  if($withoutTerms.IndexOf('ZHIZI_TEST_INVITE',[StringComparison]::OrdinalIgnoreCase) -ge 0){throw 'Public package check failed: forbidden marker ZHIZI_TEST_INVITE'}
 }
 foreach($token in $forbidden){
  foreach($content in $contents){
   if($content.IndexOf($token,[StringComparison]::OrdinalIgnoreCase) -ge 0){throw "Public package check failed: forbidden marker $token"}
  }
 }
}
Write-Output 'Public package static check passed (unpacked directory or text file list only).'
