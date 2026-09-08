# your journal for windows

The installed folder contains:

- `bin`: `journal.exe`, `solstone.exe` and their bundled helpers.
- `lib`: bundled libraries and model files.
- `share/licenses`: component licenses and model attribution.
- `share/provenance`: recorded build and input identities.

Keep these directories together.

From the installed folder, open PowerShell and run:

```powershell
.\bin\journal.exe --help
.\bin\solstone.exe --help
```

`journal` provides host commands. `solstone` provides application commands.
Model attribution is recorded in `share\licenses\models\NOTICE.md`.
