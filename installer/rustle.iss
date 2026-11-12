#define MyAppName "Rustle"
#define MyAppVersion "0.26.0"
#define MyAppPublisher "Rustle"
#define MyAppExeName "rustle.exe"
#define WebView2Client "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"

[Setup]
AppId={{8D4F6F1A-2E7B-4C2A-9F5D-6B1C0A3E7D21}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppVerName={#MyAppName} {#MyAppVersion}
DefaultDirName={localappdata}\Programs\Rustle
DefaultGroupName=Rustle
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=Rustle-{#MyAppVersion}-setup
SetupIconFile=..\crates\rustle-app\assets\rustle.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
LicenseFile=..\LICENSE
InfoAfterFile=..\README.md

[Languages]
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\rustle.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\WebView2Loader.dll"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Rustle"; Filename: "{app}\rustle.exe"
Name: "{group}\Удалить Rustle"; Filename: "{uninstallexe}"
Name: "{autodesktop}\Rustle"; Filename: "{app}\rustle.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\rustle.exe"; Description: "{cm:LaunchProgram,Rustle}"; Flags: nowait postinstall skipifsilent

[Code]
const
  WebView2Client = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';

function WebView2Installed: Boolean;
var
  Version: String;
begin
  Result :=
    RegQueryStringValue(HKCU, 'SOFTWARE\Microsoft\EdgeUpdate\Clients\' + WebView2Client, 'pv', Version) or
    RegQueryStringValue(HKLM, 'SOFTWARE\Microsoft\EdgeUpdate\Clients\' + WebView2Client, 'pv', Version) or
    RegQueryStringValue(HKLM, 'SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\' + WebView2Client, 'pv', Version);
end;

function InitializeSetup: Boolean;
begin
  Result := WebView2Installed;
  if not Result then
    MsgBox('Для работы Rustle нужен Microsoft Edge WebView2 Runtime.' + #13#10 +
           'Установите его с https://developer.microsoft.com/microsoft-edge/webview2/',
           mbError, MB_OK);
end;
