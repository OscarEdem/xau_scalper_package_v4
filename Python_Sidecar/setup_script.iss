; Script generated for XAU Scalper v4

[Setup]
AppId={{A1E2C3D4-B5F6-7890-1234-567890ABCDEF}
AppName=XAU Scalper
AppVersion=4.0
AppPublisher=Gold Trading Systems
DefaultGroupName=XAU Scalper v4
AllowNoIcons=yes
; CHANGED: Install directly to C:\XAU_Scalper to avoid Program Files permission issues
DefaultDirName=C:\XAU_Scalper
OutputDir=Installer_Build
OutputBaseFilename=XAU_Scalper_Setup_v4
SetupIconFile=icon.ico
Compression=lzma
SolidCompression=yes
WizardStyle=modern
; To prevent "Windows protected your PC" (SmartScreen) and "Unknown Publisher":
; 1. You must purchase a Code Signing Certificate (e.g., from Sectigo, DigiCert).
; 2. Configure Inno Setup: Tools > Configure Sign Tools...
; 3. Uncomment the line below and replace 'MySignTool' with your configured tool name.
; SignTool=MySignTool

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "dist\XAU_Scalper_v4.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "icon.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\XAU Scalper v4"; Filename: "{app}\XAU_Scalper_v4.exe"; IconFilename: "{app}\icon.ico"
Name: "{autodesktop}\XAU Scalper v4"; Filename: "{app}\XAU_Scalper_v4.exe"; IconFilename: "{app}\icon.ico"; Tasks: desktopicon

[Run]
Filename: "{app}\XAU_Scalper_v4.exe"; Description: "{cm:LaunchProgram,XAU Scalper v4}"; Flags: nowait postinstall skipifsilent

[Code]
function InitializeSetup(): Boolean;
begin
  Result := True;
  if MsgBox('IMPORTANT REQUIREMENT:' + #13#10 + #13#10 + 
            'This software requires MetaTrader 5 (MT5) to be installed and running.' + #13#10 + 
            'Please ensure you have an MT5 account logged in before using this bot.' + #13#10 + #13#10 + 
            'Do you want to proceed with the installation?', mbInformation, MB_YESNO) = IDNO then
  begin
    Result := False;
  end;
end;