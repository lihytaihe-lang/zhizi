#ifndef Stage
#define Stage "H:\zhizi\stage"
#endif
#ifndef Output
#define Output "H:\zhizi\output"
#endif
#ifndef Version
#define Version "0.0.0"
#endif
#ifndef Tag
#define Tag "bdev"
#endif
#ifndef LoaderDir
#define LoaderDir "H:\zhizi\stage.loader"
#endif
#ifndef Runtime
#define Runtime "H:\zhizi\deps\vc_redist.x64.exe"
#endif
[Setup]
AppId={{371930F8-3DA5-4B10-90A3-88F486C9A251}
AppName=知字拼音（测试版）
AppVersion={#Version}
AppPublisher=知字
DefaultDirName={autopf}\Zhizi
DisableDirPage=yes
DefaultGroupName=知字拼音
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#Output}
OutputBaseFilename=知字-setup-{#Version}
SetupIconFile={#Stage}\zhizi.ico
UninstallDisplayIcon={app}\{#Tag}\zhizi.ico
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=no
RestartApplications=no
ChangesAssociations=yes
UninstallDisplayName=知字拼音（测试版）

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
Source: "{#Runtime}"; DestDir: "{tmp}"; Flags: deleteafterinstall
; Every release goes into its own folder {app}\<Tag>. Windows registers only the small loader {app}\zhizi_loader_1.dll, which reads
; {app}\current.txt (written last, below) and forwards to <release folder>\zhizi_tsf.dll: apps that already hold the text service pick
; the new release up the next time they ask for it (switching input method or window) - no restart, no "restart now?". The loader file
; never changes between releases (a changed loader gets a new file name), so it is only copied when missing.
Source: "{#Stage}\*"; DestDir: "{app}\{#Tag}"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#LoaderDir}\zhizi_loader_1.dll"; DestDir: "{app}"; Flags: onlyifdoesntexist

[Icons]
Name: "{group}\关闭测试记录与上传"; Filename: "{app}\{#Tag}\zhizi-upload.exe"; Parameters: "--set-consent no"; IconFilename: "{app}\{#Tag}\zhizi.ico"
Name: "{group}\开启测试记录与上传"; Filename: "{app}\{#Tag}\zhizi-upload.exe"; Parameters: "--set-consent yes"; IconFilename: "{app}\{#Tag}\zhizi.ico"
Name: "{group}\关闭键位纠错"; Filename: "{app}\{#Tag}\zhizi-settings.exe"; Parameters: "--typo-correction off"; IconFilename: "{app}\{#Tag}\zhizi.ico"
Name: "{group}\开启键位纠错"; Filename: "{app}\{#Tag}\zhizi-settings.exe"; Parameters: "--typo-correction on"; IconFilename: "{app}\{#Tag}\zhizi.ico"

[UninstallDelete]
Type: files; Name: "{app}\current.txt"

[UninstallRun]
Filename: "{app}\{#Tag}\zhizi-upload.exe"; Parameters: "--uninstall-schedule"; Flags: runhidden waituntilterminated; RunOnceId: "RemoveUploadSchedule"
Filename: "{sys}\regsvr32.exe"; Parameters: "/u /s ""{app}\zhizi_loader_1.dll"""; Flags: runhidden waituntilterminated; RunOnceId: "UnregisterTsf"

[Code]
procedure InitializeWizard;
begin
  WizardForm.WelcomeLabel2.Caption := '知字 Windows 朋友测试版，支持 Intel/AMD x64。' + #13#10#13#10 +
    '这是测试版，已默认开启测试记录：' + #13#10 +
    '  · 输入的拼音、候选词、您选了哪一个' + #13#10 +
    '  · 光标前最近 8 个字（网址、邮箱、长串字母数字会先被替换掉；密码框不记录）' + #13#10 +
    '  · 电脑的处理器型号和电脑名称' + #13#10 +
    '这些记录每小时通过 HTTPS 上传到作者的服务器，只用来改进输入法，不会给别人。' + #13#10 +
    '随时可在开始菜单“知字拼音”里点“关闭测试记录与上传”。' + #13#10#13#10 +
    '安装后在输入法列表选择“知字拼音”。卸载会撤销上传任务，但保留您的个人词库。';
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var Code: Integer;
begin
  Result := '';
  ExtractTemporaryFile('vc_redist.x64.exe');
  if not Exec(ExpandConstant('{tmp}\vc_redist.x64.exe'), '/install /quiet /norestart', '', SW_HIDE, ewWaitUntilTerminated, Code) then
    Result := 'Cannot start Microsoft Visual C++ runtime installer.'
  else if (Code <> 0) and (Code <> 1638) and (Code <> 3010) then
    Result := 'Microsoft Visual C++ runtime installation failed: ' + IntToStr(Code);
  if Code = 3010 then NeedsRestart := True;
end;

// Removes the folders of earlier releases and the loose files of the first test package, which kept everything
// directly in {app}. Anything an app still holds open stays until the next upgrade; that is harmless.
procedure RemoveEarlierReleases(Previous: String);
var Find: TFindRec; Name: String;
begin
  if FindFirst(ExpandConstant('{app}\*'), Find) then
  try
    repeat
      Name := Find.Name;
      if (Name <> '.') and (Name <> '..') and (Name <> '{#Tag}') and (Name <> Previous) then begin
        if (Find.Attributes and FILE_ATTRIBUTE_DIRECTORY) <> 0 then begin
          if (Length(Name) > 1) and (Name[1] = 'b') then DelTree(ExpandConstant('{app}\') + Name, True, True, True)
          else if (Name = 'Model') or (Name = 'Rime') or (Name = 'Licenses') then DelTree(ExpandConstant('{app}\') + Name, True, True, True);
        end else if (Name <> 'current.txt') and (Pos('zhizi_loader', Name) <> 1) and (Pos('unins', Name) <> 1) then DeleteFile(ExpandConstant('{app}\') + Name);
      end;
    until not FindNext(Find);
  finally
    FindClose(Find);
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var Code: Integer; Answer: String; Previous: AnsiString;
begin
  if CurStep = ssPostInstall then begin
    // the release that was current until now stays on disk too: apps still running it load its Model/Rime lazily
    Previous := '';
    if FileExists(ExpandConstant('{app}\current.txt')) then LoadStringFromFile(ExpandConstant('{app}\current.txt'), Previous);
    Previous := Trim(Previous);
    if not SaveStringToFile(ExpandConstant('{app}\current.txt'), '{#Tag}', False) then
      RaiseException('Cannot write current.txt.');
    if not Exec(ExpandConstant('{sys}\icacls.exe'), ExpandConstant('"{app}" /grant *S-1-15-2-1:(OI)(CI)RX /T /Q'), '', SW_HIDE, ewWaitUntilTerminated, Code) or (Code <> 0) then
      RaiseException('Cannot set application package read access.');
    if not Exec(ExpandConstant('{sys}\regsvr32.exe'), ExpandConstant('/s "{app}\zhizi_loader_1.dll"'), '', SW_HIDE, ewWaitUntilTerminated, Code) or (Code <> 0) then
      RaiseException('TSF registration failed. Exit code: ' + IntToStr(Code));
    // Recording starts on (the owner tells testers in person); /CONSENT=no installs it switched off.
    Answer := 'yes';
    if ExpandConstant('{param:CONSENT|}') = 'no' then Answer := 'no';
    if not ExecAsOriginalUser(ExpandConstant('{app}\{#Tag}\zhizi-upload.exe'), '--set-consent ' + Answer, '', SW_HIDE, ewWaitUntilTerminated, Code) or (Code <> 0) then
      RaiseException('Saving the recording choice failed. Exit code: ' + IntToStr(Code));
    RemoveEarlierReleases(String(Previous));
  end;
end;
