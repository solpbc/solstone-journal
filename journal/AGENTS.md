# this journal

This directory is a journal: a folder, organized by day, that the solstone app on the owner's devices adds to. Work with it through the `solstone` and `journal` command-line tools rather than by editing files. On a mac, the journal app's admin terminal provides them.

- `solstone call ...` reads and changes what the journal holds. Start with `solstone help`, then `solstone call <app> --help`, and add `--help` to a command for its flags.
- `journal ...` runs and checks on the journal on this machine, for example `journal health` and `journal talent`.

Setup installs router skills at `./.claude/skills/{journal,solstone}/` and `./.agents/skills/{journal,solstone}/`. For the journal layout and the `solstone call journal` reference, start with the `journal` skill's `SKILL.md`, then use `references/cli.md`, `references/config.md`, `references/facets.md`, `references/captures.md`, `references/logs.md` and `references/storage.md`.
