# Copies the TAP next to the exe under the name `taskbar::enable()` loads.
#
# The spike builds `xaml_tap.dll`; the app looks for `audio_tray_tap.dll`. Run
# from the repo root (the "stage TAP dll" task sets cwd).

$src = 'spikes/xaml-tap/target/debug/xaml_tap.dll'
$dst = 'target/debug/audio_tray_tap.dll'

if (-not (Test-Path $src)) {
    Write-Host "TAP not built - $src is missing; the taskbar toggle will report the DLL as absent."
    exit 0
}

try {
    Copy-Item $src $dst -Force -ErrorAction Stop
    Write-Host "staged $dst"
} catch {
    # Explorer pins the TAP once injected, so the file cannot be replaced while a
    # previous one is loaded. Not fatal: the DLL already on disk is still
    # injectable, and failing here would block launching altogether. Restart
    # Explorer to pick up a rebuilt TAP.
    Write-Host "could not replace $dst - Explorer has a TAP loaded; restart Explorer to update it."
}
