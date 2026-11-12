; Windows installer for RTerm (Inno Setup 6).
; Built by scripts/bundle-windows.ps1:
;   ISCC /DAppVersion=1.2.3 /DSourceExe=path\to\RTerm.exe /DOutputDir=dist /DOutputName=... packaging\windows\rterm.iss
;
; Installs per user by default (no administrator prompt) into
; %LOCALAPPDATA%\Programs\RTerm. Settings and history live in
; %USERPROFILE%\.config\rterm and are kept on uninstall.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\..\target\release\rterm.exe"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif
#ifndef OutputName
  #define OutputName "RTerm-Setup"
#endif

[Setup]
AppId={{204EAA76-CABA-452E-B884-2B04CD5B3268}
AppName=RTerm
AppVersion={#AppVersion}
AppVerName=RTerm {#AppVersion}
AppPublisher=Rawz06 Studio
AppPublisherURL=https://github.com/Rawz06-Studio/RTerm
AppSupportURL=https://github.com/Rawz06-Studio/RTerm/issues
AppUpdatesURL=https://github.com/Rawz06-Studio/RTerm/releases
DefaultDirName={autopf}\RTerm
DefaultGroupName=RTerm
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile=..\..\LICENSE
SetupIconFile=..\..\assets\icon.ico
UninstallDisplayIcon={app}\RTerm.exe
UninstallDisplayName=RTerm
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
ShowLanguageDialog=auto

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "french"; MessagesFile: "compiler:Languages\French.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "RTerm.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\RTerm"; Filename: "{app}\RTerm.exe"
Name: "{autodesktop}\RTerm"; Filename: "{app}\RTerm.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\RTerm.exe"; Description: "{cm:LaunchProgram,RTerm}"; Flags: nowait postinstall skipifsilent
