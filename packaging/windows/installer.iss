; ZigZag's Windows installer (Inno Setup 6.3 or later).
;
; Built by .github/workflows/windows-release.yml from the same staged folder
; that becomes the zip:
;
;   ISCC /DAppVersion=0.1.0 /DSourceDir=C:\...\ZigZag-0.1.0-x86_64-windows ^
;        /DIconFile=C:\...\AppIcon.ico /DOutputDir=C:\...\dist ^
;        /DOutputName=ZigZag-0.1.0-x86_64-windows-setup installer.iss
;
; It installs for the current user without asking for administrator rights
; (into %LOCALAPPDATA%\Programs\ZigZag); /ALLUSERS on the command line
; installs for everyone instead. It adds a Start menu shortcut, the
; zigzag:// link the share pages open, ZigZag in Open with for the files it
; reads (without taking any of them over), and an uninstaller. Putting
; zigzag-cli on the PATH and a desktop shortcut are choices.

#ifndef AppVersion
  #error Pass /DAppVersion=<version>
#endif
#ifndef SourceDir
  #error Pass /DSourceDir=<staged release folder>
#endif
#ifndef IconFile
  #define IconFile "AppIcon.ico"
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif
#ifndef OutputName
  #define OutputName "ZigZag-" + AppVersion + "-x86_64-windows-setup"
#endif

[Setup]
; Identifies ZigZag to Windows across versions: never change it.
AppId={{8A3F5864-AD0F-4B12-B0E0-D922FDFD53E9}
AppName=ZigZag
AppVersion={#AppVersion}
AppVerName=ZigZag {#AppVersion}
AppPublisher=ZigZag
AppPublisherURL=https://zigzag.diy
AppSupportURL=https://github.com/simpsoka/zigzag/issues
AppUpdatesURL=https://github.com/simpsoka/zigzag/releases/latest
VersionInfoVersion={#AppVersion}
DefaultDirName={autopf}\ZigZag
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Windows 10 version 1709, which Bluetooth to the Skitch needs.
MinVersion=10.0.16299
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\ZigZag.exe
UninstallDisplayName=ZigZag
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
ChangesEnvironment=yes
ChangesAssociations=yes
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked
Name: "addtopath"; Description: "Add zigzag-cli to the PATH, for use in a terminal"; Flags: unchecked

[Files]
Source: "{#SourceDir}\ZigZag.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\zigzag-cli.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\zigzag-mcp.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\NOTICE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\THIRD-PARTY-NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\README.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\fonts\*"; DestDir: "{app}\fonts"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\ZigZag"; Filename: "{app}\ZigZag.exe"
Name: "{autodesktop}\ZigZag"; Filename: "{app}\ZigZag.exe"; Tasks: desktopicon

[Registry]
; zigzag:// links, the same keys ZigZag.exe writes for itself when it starts
; (src/studio/link.rs), so Open on a share page works before the first run.
Root: HKA; Subkey: "Software\Classes\zigzag"; ValueType: string; ValueName: ""; ValueData: "URL:ZigZag"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\zigzag"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\zigzag\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\ZigZag.exe"",0"
Root: HKA; Subkey: "Software\Classes\zigzag\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\ZigZag.exe"" ""%1"""
; Installed for everyone, ZigZag.exe still writes the link under the user
; who runs it: take that away too when uninstalling.
Root: HKCU; Subkey: "Software\Classes\zigzag"; Flags: dontcreatekey uninsdeletekey; Check: IsAdminInstallMode

; Open with: ZigZag is offered for the files it reads and stays out of the
; way of whatever opens them now.
Root: HKA; Subkey: "Software\Classes\Applications\ZigZag.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "ZigZag"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\ZigZag.exe\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\ZigZag.exe"" ""%1"""
Root: HKA; Subkey: "Software\Classes\Applications\ZigZag.exe\SupportedTypes"; ValueType: string; ValueName: ".svg"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\ZigZag.exe\SupportedTypes"; ValueType: string; ValueName: ".pes"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\ZigZag.exe\SupportedTypes"; ValueType: string; ValueName: ".pec"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\ZigZag.exe\SupportedTypes"; ValueType: string; ValueName: ".dst"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\ZigZag.exe\SupportedTypes"; ValueType: string; ValueName: ".exp"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\ZigZag.exe\SupportedTypes"; ValueType: string; ValueName: ".jef"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\ZigZag.exe\SupportedTypes"; ValueType: string; ValueName: ".vp3"; ValueData: ""

[Run]
Filename: "{app}\ZigZag.exe"; Description: "Open ZigZag"; Flags: nowait postinstall skipifsilent

[Code]
function EnvRoot: Integer;
begin
  if IsAdminInstallMode then
    Result := HKEY_LOCAL_MACHINE
  else
    Result := HKEY_CURRENT_USER;
end;

function EnvKey: String;
begin
  if IsAdminInstallMode then
    Result := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment'
  else
    Result := 'Environment';
end;

{ Puts Dir on the PATH (once) or takes it off, leaving the rest as it was. }
procedure SetOnPath(Dir: String; Wanted: Boolean);
var
  Path: String;
  P: Integer;
begin
  if not RegQueryStringValue(EnvRoot, EnvKey, 'Path', Path) then
    Path := '';
  P := Pos(';' + Lowercase(Dir) + ';', ';' + Lowercase(Path) + ';');
  if Wanted then
  begin
    if P > 0 then
      Exit;
    if (Path <> '') and (Path[Length(Path)] <> ';') then
      Path := Path + ';';
    Path := Path + Dir;
  end
  else
  begin
    if P = 0 then
      Exit;
    { Dir starts at Path[P]. Take it and one separator: the one after it,
      or the one before when it is the last entry. }
    if (P + Length(Dir) > Length(Path)) and (P > 1) then
      Delete(Path, P - 1, Length(Dir) + 1)
    else
      Delete(Path, P, Length(Dir) + 1);
  end;
  RegWriteExpandStringValue(EnvRoot, EnvKey, 'Path', Path);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    SetOnPath(ExpandConstant('{app}'), WizardIsTaskSelected('addtopath'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    SetOnPath(ExpandConstant('{app}'), False);
end;
