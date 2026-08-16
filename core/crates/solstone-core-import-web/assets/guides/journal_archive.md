# Exporting Your Journal

If you're moving a journal between machines, create a `.zip` archive and bring it here.

## On this machine

If your other journal is on this same machine:

1. Open Terminal in the source checkout.
2. Create an archive:

   ```bash
   cd /path/to/source/solstone
   zip -r ~/Downloads/journal.zip ./journal
   ```

3. Upload that `.zip` here.

### About sol-transfer

`journal transfer` moves raw observations between machines — it doesn't move your merged journal, facets, entities, or import history. Create a journal archive instead.

## From another machine

1. On the source machine, create a journal archive with `zip -r journal.zip ./journal` from the source checkout.
2. Move the `.zip` to this machine however you'd normally move a file.
3. Upload it under the Journal card on this screen.

## Manual fallback

The native journal export command is temporarily unavailable while archive support migrates. You can `zip` the journal directly:

```bash
cd /path/to/source/solstone
zip -r journal.zip ./journal
```

The importer accepts either a direct journal-root archive or a single wrapper folder containing `chronicle/`, `entities/`, `facets/`, and `imports/`.
