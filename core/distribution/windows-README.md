# your journal for windows

The installed folder contains:

- `bin`: `solstone.exe` and its bundled helpers.
- `lib`: bundled libraries and model files.
- `share/licenses`: component licenses and model attribution.
- `share/provenance`: recorded build and input identities.

Keep these directories together.

From the installed folder, open PowerShell and run:

```powershell
.\bin\solstone.exe --help
.\bin\solstone.exe journal --help
```

`solstone journal` runs the journal on this computer, and the other `solstone`
commands reach your journal through its API. `journal <args>` has moved to `solstone journal <args>`; update any commands or scripts that use the old name.
Model attribution is recorded in `share\licenses\models\NOTICE.md`.
