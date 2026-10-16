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
; The taskbar TAP, loaded into explorer.exe to draw the taskbar strip. Optional:
; without it the app falls back to the plain tray icon and logs the DLL as missing.
#ifndef SourceTap
  #define SourceTap "..\target\release\audio_tray_tap.dll"
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
; `skipifsourcedoesntexist` so a build without the TAP still produces an installer.
; Explorer keeps the DLL loaded once the strip has been injected, so it cannot be
; overwritten. `restartreplace` cannot help: its boot-time rename needs administrator
; rights, which a per-user install does not have. `SetAsideLoadedTap` renames the
; held file out of the way instead; Explorer runs the old image until it restarts.
; `uninsrestartdelete` likewise only takes effect for an elevated (all-users) install;
; per-user, a DLL Explorer still holds at uninstall is left behind.
Source: "{#SourceTap}"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist uninsrestartdelete; BeforeInstall: SetAsideLoadedTap

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExeName}"
; Startup-folder shortcut (removed automatically on uninstall) — only if the
; user keeps the "autostart" task ticked.
Name: "{userstartup}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: autostart

[Run]
; Offer to launch right after an interactive install; skipped for silent
; (winget / CI) installs.
Filename: "{app}\{#AppExeName}"; Description: "Launch {#AppName} now"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; TAPs set aside by `SetAsideLoadedTap` or the self-updater (`update::install_tap`).
Type: files; Name: "{app}\audio_tray_tap.dll.old*"

[Code]
// An image Explorer has loaded cannot be overwritten or deleted, but it can be
// renamed, unelevated. Move a held TAP to the first free `.old<n>` slot (the
// names `update::install_tap` uses) so the new one lands under the real name.
// A TAP nothing holds is simply deleted.
procedure SetAsideLoadedTap();
var
  Tap, Aside: String;
  Slot: Integer;
begin
  Tap := ExpandConstant('{app}\audio_tray_tap.dll');
  if not FileExists(Tap) or DeleteFile(Tap) then
    Exit;
  for Slot := 0 to 3 do
  begin
    Aside := Tap + '.old' + IntToStr(Slot);
    if FileExists(Aside) and not DeleteFile(Aside) then
      Continue;
    if RenameFile(Tap, Aside) then
      Log('Set aside the loaded TAP as ' + Aside)
    else
      Log('Could not set aside the loaded TAP; the copy will report it in use');
    Exit;
  end;
end;
