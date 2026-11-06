# WebView2Loader.dll

`assets/webview2/x64/WebView2Loader.dll` is redistributed as part of
the Microsoft Edge WebView2 SDK so that GNU-toolchain builds can load
WebView2 at runtime.

Copyright (c) Microsoft Corporation. Use is subject to the Microsoft
WebView2 SDK license terms:
https://learn.microsoft.com/microsoft-edge/webview2/

The DLL is architecture-specific (x64) and must be shipped next to
`rustle.exe`. The build script of `rustle-app` copies it into the cargo
target directory for development builds.
