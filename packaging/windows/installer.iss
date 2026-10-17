; Inno Setup script for the Windows installer. The release workflow runs:
;   iscc /DAppVersion=<version> /DSourceDir=<folder with the exe> packaging\windows\installer.iss
; It installs for the current user only (no administrator rights needed),
; adds a Start menu entry and opens .mtp files with the app.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\target\release"
#endif

[Setup]
AppId={{6A2E3B1C-7D45-4E8B-9C31-5F0A2D9E4B71}
AppName=Modular Terrain Planner
AppVersion={#AppVersion}
AppPublisher=HaXrDEV
AppPublisherURL=https://github.com/HaXrDEV/modular-terrain-planner
DefaultDirName={localappdata}\Programs\Modular Terrain Planner
DefaultGroupName=Modular Terrain Planner
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=..\..\dist
OutputBaseFilename=ModularTerrainPlanner-{#AppVersion}-windows-x64-setup
SetupIconFile=..\..\assets\icons\app_icon.ico
UninstallDisplayIcon={app}\modular-terrain-planner.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
ChangesAssociations=yes

[Files]
Source: "{#SourceDir}\modular-terrain-planner.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\THIRD_PARTY_LICENSES.html"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\..\assets\fonts\OFL.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Modular Terrain Planner"; Filename: "{app}\modular-terrain-planner.exe"

[Registry]
Root: HKCU; Subkey: "Software\Classes\.mtp"; ValueType: string; ValueName: ""; ValueData: "ModularTerrainPlanner.Project"; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\ModularTerrainPlanner.Project"; ValueType: string; ValueName: ""; ValueData: "Modular Terrain Planner project"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\ModularTerrainPlanner.Project\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\modular-terrain-planner.exe,0"
Root: HKCU; Subkey: "Software\Classes\ModularTerrainPlanner.Project\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\modular-terrain-planner.exe"" ""%1"""

[Run]
Filename: "{app}\modular-terrain-planner.exe"; Description: "Start Modular Terrain Planner"; Flags: nowait postinstall skipifsilent
