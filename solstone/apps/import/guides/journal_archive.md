# Exporting Your Journal

If you're moving a journal between machines, export it as a `.zip` and bring it here.

## On this machine

If your other journal is on this same machine:

1. Open Terminal.
2. Run:

   ```bash
   sol call journal export --out ~/Downloads/journal.zip
   ```

3. Wait for the command to print the archive path.
4. Upload that `.zip` here.

### About sol-transfer

`journal transfer` moves raw observations between machines — it doesn't move your merged journal, facets, entities, or import history. Use `sol call journal export` for that.

## From another machine

1. On the source machine, run `sol call journal export --out <path>`.
2. Move the `.zip` to this machine however you'd normally move a file.
3. Upload it under the Journal card on this screen.

## Manual fallback

If the source machine doesn't have `sol call journal export`, you can `zip` the journal directly:

```bash
cd /path/to/source/solstone
zip -r journal.zip ./journal
```

The importer accepts either a direct journal-root archive or a single wrapper folder containing `chronicle/`, `entities/`, `facets/`, and `imports/`.
