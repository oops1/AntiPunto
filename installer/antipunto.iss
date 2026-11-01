; Установщик AntiPunto: на пользователя, без прав администратора.
; Сборка: ISCC /DAppVersion=0.1.0 installer\antipunto.iss (после scripts\make-dist.ps1)

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{ED76DD0B-E389-4082-84D5-95CD0AC6EB18}
AppName=AntiPunto
AppVersion={#AppVersion}
AppVerName=AntiPunto {#AppVersion}
AppPublisher=oops1
AppPublisherURL=https://github.com/oops1/AntiPunto
AppSupportURL=https://github.com/oops1/AntiPunto/issues
AppUpdatesURL=https://github.com/oops1/AntiPunto/releases
DefaultDirName={localappdata}\Programs\AntiPunto
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
OutputBaseFilename=AntiPunto-{#AppVersion}-setup
SetupIconFile=..\client\assets\app.ico
UninstallDisplayIcon={app}\antipunto.exe
UninstallDisplayName=AntiPunto
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; то же имя мьютекса, что у программы: установщик закроет запущенную копию
AppMutex=AntiPunto_single
CloseApplications=force
RestartApplications=no

[Languages]
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
ru.Autostart=Запускать при входе в Windows
en.Autostart=Start when I sign in to Windows
ru.LaunchNow=Запустить AntiPunto
en.LaunchNow=Launch AntiPunto

[Tasks]
Name: "autostart"; Description: "{cm:Autostart}"

[Files]
Source: "..\dist\AntiPunto\*"; DestDir: "{app}"; Flags: recursesubdirs ignoreversion

[Icons]
Name: "{userprograms}\AntiPunto"; Filename: "{app}\antipunto.exe"

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "AntiPunto"; ValueData: """{app}\antipunto.exe"""; Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{app}\antipunto.exe"; Description: "{cm:LaunchNow}"; Flags: nowait postinstall

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/IM antipunto.exe /F"; Flags: runhidden; RunOnceId: "StopApp"

[UninstallDelete]
Type: filesandordirs; Name: "{localappdata}\AntiPunto"
