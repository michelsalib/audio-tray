; Inno Setup script for audio-tray.
;
; Per-user install (no administrator rights) so the in-app self-updater can
; replace the exe in place. Built by CI (.github/workflows/release.yml), which
; passes the version and the freshly built exe on the command line:
;
;   ISCC.exe /DAppVersion=0.1.1 "/DSourceExe=<abs path>\audio-tray.exe" packaging\audio-tray.iss
;
; Output: packaging\dist\AudioTray-<AppVersion>-Setup.exe

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\target\release\audio-tray.exe"
#endif

#define AppName "Audio Tray"
#define AppPublisher "MichelSalib"
#define AppUrl "https://github.com/michelsalib/audio-tray"
#define AppExeName "audio-tray.exe"

[Setup]
; AppId must stay CONSTANT across versions so upgrades replace instead of
; installing side-by-side. Do not change this GUID.
AppId={{F729DFE9-9049-41FE-A134-74EA86296F85}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}/issues
AppUpdatesURL={#AppUrl}/releases
; Per-user, no elevation. Users may still opt into an all-users install via the
; UI, but the default (and winget scope) is per-user.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
DefaultDirName={autopf}\AudioTray
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\{#AppExeName}
UninstallDisplayName={#AppName}
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=dist
OutputBaseFilename=AudioTray-{#AppVersion}-Setup
; Icon for the Setup.exe itself (the installed exe already embeds it via build.rs).
SetupIconFile=..\assets\app.ico
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; The tray runs continuously; close it (via Restart Manager) before overwriting
; the exe so re-install / winget upgrade doesn't fail with a file lock.
CloseApplications=yes
CloseApplicationsFilter={#AppExeName}
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "autostart"; Description: "Start {#AppName} automatically when I sign in"; GroupDescription: "Startup:"

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExeName}"
; Startup-folder shortcut (removed automatically on uninstall) — only if the
; user keeps the "autostart" task ticked.
Name: "{userstartup}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: autostart

[Run]
; Offer to launch right after an interactive install; skipped for silent
; (winget / CI) installs.
Filename: "{app}\{#AppExeName}"; Description: "Launch {#AppName} now"; Flags: nowait postinstall skipifsilent
