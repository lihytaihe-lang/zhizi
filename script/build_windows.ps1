param([string]$Root='H:\zhizi',[switch]$Package,[string]$StageDir='')
if(!$StageDir){$StageDir=Join-Path $Root ('stage-'+(Get-Date -Format 'yyyyMMdd-HHmmss'))}
$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.Encoding]::UTF8
$env:CARGO_TARGET_DIR="$Root\target"
$env:CARGO_HOME="$Root\cargo"
$env:TEMP="$Root\temp"; $env:TMP=$env:TEMP
$env:RIME_ROOT="$Root\deps\librime-1.16.1\dist"
New-Item -ItemType Directory -Force $env:TEMP,$env:CARGO_HOME,"$StageDir","$Root\output" | Out-Null
$manifest="$Root\src\native\windows\zhizi-tsf\Cargo.toml"
cargo build --release --locked --manifest-path $manifest
if($LASTEXITCODE -ne 0){throw 'TSF build failed'}
cargo build --release --manifest-path "$Root\src\native\windows\zhizi-loader\Cargo.toml"
if($LASTEXITCODE -ne 0){throw 'Loader build failed'}
if([string]::IsNullOrWhiteSpace($env:ZHIZI_TEST_INVITE)){throw 'Missing ZHIZI_TEST_INVITE (never put it in source)'}
try {
 cargo build --release --features upload --bin zhizi-upload --manifest-path "$Root\src\native\host\Cargo.toml"
 if($LASTEXITCODE -ne 0){throw 'Uploader build failed'}
} finally {Remove-Item Env:ZHIZI_TEST_INVITE -ErrorAction SilentlyContinue}
cargo build --release --bin zhizi-settings --manifest-path "$Root\src\native\host\Cargo.toml"
if($LASTEXITCODE -ne 0){throw 'Settings tool build failed'}
Copy-Item "$Root\target\release\zhizi_tsf.dll","$Root\target\release\zhizi-upload.exe","$Root\target\release\zhizi-settings.exe","$env:RIME_ROOT\lib\rime.dll" "$StageDir" -Force
Copy-Item "$Root\deps\onnxruntime\onnxruntime.dll" "$StageDir" -Force
# the loader is installed beside the release folders (not in one): it is the DLL Windows registers and it never changes
$loaderDir="$StageDir.loader"; New-Item -ItemType Directory -Force $loaderDir | Out-Null
Copy-Item "$Root\target\release\zhizi_loader.dll" "$loaderDir\zhizi_loader_1.dll" -Force
Copy-Item "$Root\src\native\windows\zhizi-tsf\resources\zhizi.ico" "$StageDir" -Force
New-Item -ItemType Directory -Force "$StageDir\Model","$StageDir\Rime" | Out-Null
Copy-Item "$Root\model\zhizi30m_v2.onnx","$Root\model\itos.json" "$StageDir\Model" -Force
# The dictionaries come from the synced source tree, the same files the Mac bundle ships (script/build_native.sh). Until 2026-10-17
# they came from a hand-made copy in $Root\shared\Rime that stopped at 10-16, so the 0.1.3 package first built missed that day's
# lexicon. Only the pinned upstream base table (pinyin_simp, not in the repository) still comes from there.
$rimeFiles='default.yaml','agent_pinyin.schema.yaml','agent_pinyin.dict.yaml','agent_common_fix.dict.yaml','agent_daily.dict.yaml',
 'agent_english.dict.yaml','agent_project_terms.dict.yaml','agent_tech_terms.dict.yaml','agent_ai_terms.dict.yaml','agent_pack_terms.dict.yaml',
 'agent_idioms.dict.yaml','agent_thuocl_it.dict.yaml','agent_thuocl_caijing.dict.yaml','agent_modern.dict.yaml','agent_general.dict.yaml',
 'agent_newwords.dict.yaml','agent_spoken.dict.yaml','agent_ai_names.dict.yaml','agent_pack_poetry.dict.yaml','agent_pack_cad.dict.yaml',
 'agent_pack_industry.dict.yaml','agent_exact_readings.tsv'
foreach($f in $rimeFiles){Copy-Item "$Root\src\native\resources\Rime\$f" "$StageDir\Rime\$f" -Force}
Copy-Item "$Root\shared\Rime\pinyin_simp.dict.yaml" "$StageDir\Rime\pinyin_simp.dict.yaml" -Force
$missing=Get-ChildItem "$Root\src\native\resources\Rime" -Filter '*.dict.yaml' | Where-Object {$rimeFiles -notcontains $_.Name}
if($missing){throw ('Rime files not staged: '+($missing.Name -join ', '))}
New-Item -ItemType Directory -Force "$StageDir\Licenses" | Out-Null
Copy-Item "$Root\src\native\windows\licenses\*" "$StageDir\Licenses" -Force
$manifestFiles=Get-ChildItem "$StageDir" -Recurse -File | Where-Object {$_.Name -ne 'SHA256SUMS.txt'} | ForEach-Object {
 $hash=Get-FileHash $_.FullName -Algorithm SHA256
 "$($hash.Hash.ToLower())  $($_.FullName.Substring(($StageDir+'\').Length))"
}
$manifestFiles | Set-Content "$StageDir\SHA256SUMS.txt" -Encoding UTF8
if($Package){
 $runtime="$Root\deps\vc_redist.x64.exe"
 if(!(Test-Path $runtime)){throw 'Missing official Microsoft VC x64 redistributable in deps'}
 $signature=Get-AuthenticodeSignature $runtime
 if($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation'){throw 'Invalid Microsoft runtime signature'}

 $tag='b'+(Get-Date -Format 'yyyyMMddHHmmss')
 $ver=(Get-Content -Raw "$Root\src\VERSION").Trim()
 if($ver -notmatch '^\d+\.\d+\.\d+$'){throw "bad VERSION: $ver"}
 & "$Root\tools\InnoSetup\ISCC.exe" "/DTag=$tag" "/DVersion=$ver" "/DStage=$StageDir" "/DOutput=$Root\output" "/DRuntime=$runtime" "/DLoaderDir=$loaderDir" "$Root\src\native\windows\installer\zhizi.iss"
 if($LASTEXITCODE -ne 0){throw 'Installer build failed'}
}
