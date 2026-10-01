# Microsoft Edge WebView2 loader

the journal app for windows embeds the Microsoft Edge WebView2 loader in its
executable. the loader is linked statically from WebView2LoaderStatic.lib in
Microsoft's WebView2 SDK, NuGet package Microsoft.Web.WebView2 version
1.0.3650.58.

Microsoft licenses the loader under the BSD 3-Clause license reproduced below.
that license, not solstone's AGPL-3.0-only license, governs this component.
Microsoft does not sponsor or endorse solstone.

Source:

- Executable: bin/journal-app.exe
- Library: x64/WebView2LoaderStatic.lib in the webview2-com-sys 0.38.2 crate
  (SHA-256 0659b741bde6348d4c4a6ec4ceb9af50e3d0048ed9cd3c8659bccbb61fde55ee),
  byte-identical to build/native/x64/WebView2LoaderStatic.lib in
  Microsoft.Web.WebView2 1.0.3650.58 (package SHA-256
  911a472128c82ac8baa0c486c23342cc9dd6e7dc50d754e676726642ca065c60)
- License text: LICENSE.txt in that NuGet package, with trailing spaces removed
- The webview2-com-sys crate is MIT-licensed; that license does not cover
  this library

Copyright (C) Microsoft Corporation. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

   * Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.
   * Redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with the
distribution.
   * The name of Microsoft Corporation, or the names of its contributors
may not be used to endorse or promote products derived from this
software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
