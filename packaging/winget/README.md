# windows journal on winget

The journal uses `solpbc.Journal`, with moniker `solstone-journal`. The solstone
app uses its own package, `solpbc.Solstone`; neither depends on the other.

The manifests install the production-signed, per-user Velopack Setup from the
versioned URL on `updates.solstone.app`. A silent install puts the program in
place. Open **journal** from the Start menu to set it up or start it.
WebView2 is the journal app's runtime dependency.

After publishing a windows release, prepare manifests from the published bytes:

```sh
uv run scripts/winget.py prepare 2.0.38 --release-date 2026-10-09
```

Replace the version and date with the release being submitted. Commit all three
YAML files together, then copy them into
`manifests/s/solpbc/Journal/<version>/` in a fork of
[`microsoft/winget-pkgs`](https://github.com/microsoft/winget-pkgs).
Run `winget validate --manifest <directory>` on windows and submit one
package/version per pull request. Check for an existing journal submission first;
update that submission rather than creating a duplicate. The package-manager
submission follows origin publication and does not hold that release.

Check the committed version, complete installer hash and merged channel:

```sh
uv run scripts/winget.py check
```

`CURRENT` means the merged manifest has this version, installer URL, SHA-256
and architecture. `PENDING` names an open PR and exits nonzero;
`--allow-pending` permits that state while still reporting it as pending.
`MISSING`, stale manifests, hash mismatches and failed API reads fail the check.
An open PR still needs its upstream validation checks and moderator review.

Velopack uses the stable `SolstoneJournal` uninstall key for detection, and
registers its own quiet uninstall command. Rerunning Setup stops processes from
the program folder before replacing it. A silent install does not open the
journal app. The journal's files must live outside the program folder.
Journal setup refuses an overlapping location.

Microsoft's [first contribution checklist](https://github.com/microsoft/winget-pkgs/blob/master/doc/FirstContribution.md)
and [validation guide](https://github.com/microsoft/winget-pkgs/blob/master/doc/Validation.md)
describe the upstream gates.
