; rterm — Inno Setup 6 安装包脚本（Sprint 11 / 11.4）
;
; 生成安装包：
;   1) cargo build --release -p rterm-app
;   2) iscc installer\rterm.iss
;   3) 产物在 dist\rterm-1.0.0-setup.exe
;
; 说明：rterm 没有自定义文件格式，因此不做文件关联；若后续需要，在 [Registry] 增加即可。
;       有图标后取消 SetupIconFile / IconFilename 的注释。

#define AppName "rterm"
#define AppVersion "1.0.0"
#define AppPublisher "rterm"
#define AppExeName "rterm-app.exe"

[Setup]
; 固定 AppId，保证升级安装时复用同一卸载项
AppId={{7C2B0A46-59B5-4D9C-9B3E-4A9D1C1F6E31}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
OutputDir=..\dist
OutputBaseFilename=rterm-{#AppVersion}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayName={#AppName}
; SetupIconFile=..\assets\rterm.ico

[Languages]
Name: "chinesesimplified"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加任务:"; Flags: unchecked

[Files]
Source: "..\target\release\{#AppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExeName}"
Name: "{group}\卸载 {#AppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExeName}"; Description: "启动 {#AppName}"; Flags: nowait postinstall skipifsilent
