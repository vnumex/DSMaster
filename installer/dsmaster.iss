; DSMaster Windows Setup Installer (Inno Setup 6)
; Build locally:
;   iscc installer\dsmaster.iss
; Or with custom overrides:
;   iscc /DAppVersion=0.2.1 /DAppTag=v0.2.1 /DExePath=D:\DSMaster\target\release\dsmaster.exe /DOutputDir=D:\DSMaster\dist installer\dsmaster.iss

#ifndef AppVersion
  #define AppVersion "0.3.2"
#endif
#ifndef AppTag
  #define AppTag "v" + AppVersion
#endif
#ifndef ExePath
  #define ExePath "..\target\release\dsmaster.exe"
#endif
#ifndef OutputDir
  #define OutputDir "..\dist"
#endif

[Setup]
AppId={{8E5F3D74-2C6A-4B9E-A917-5C6D0D2B8E1A}
AppName=DSMaster
AppVersion={#AppVersion}
AppVerName=DSMaster {#AppVersion}
AppPublisher=tassak-dozu
AppPublisherURL=https://github.com/tassak-dozu/DSMaster
AppSupportURL=https://github.com/tassak-dozu/DSMaster/issues
AppUpdatesURL=https://github.com/tassak-dozu/DSMaster/releases
DefaultDirName={autopf}\DSMaster
DisableDirPage=no
DisableProgramGroupPage=yes
LicenseFile=..\LICENSE
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
OutputDir={#OutputDir}
OutputBaseFilename=DSMaster-{#AppTag}-windows-x64-setup
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\dsmaster.exe
UninstallDisplayName=DSMaster
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
AppMutex=Local\DSMaster_SingleInstance_v1
CloseApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "startmenuicon"; Description: "Create a &Start Menu shortcut"; GroupDescription: "Additional shortcuts:"
Name: "desktopicon"; Description: "Create a &Desktop shortcut"; GroupDescription: "Additional shortcuts:"

[Files]
Source: "{#ExePath}"; DestDir: "{app}"; DestName: "dsmaster.exe"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\DSMaster"; Filename: "{app}\dsmaster.exe"; Tasks: startmenuicon
Name: "{autodesktop}\DSMaster"; Filename: "{app}\dsmaster.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\dsmaster.exe"; Description: "Launch DSMaster"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
Type: files; Name: "{app}\dsmaster.exe.old"
