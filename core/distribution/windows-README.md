# your journal for windows

The installed folder contains:

- `bin`: `journal.exe`, `solstone.exe` and their bundled helpers.
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
commands read your journal through its API. `journal` is a shorter name for
`solstone journal`.
Model attribution is recorded in `share\licenses\models\NOTICE.md`.
