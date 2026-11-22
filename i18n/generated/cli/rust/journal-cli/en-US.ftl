m6a6f75726e616c2e636c692e61626f75742e6f732e72656c656173652e756e617661696c61626c65 = about: { "os-release" } unavailable: { $error }

m6a6f75726e616c2e636c692e61626f75742e7265736f757263652e756e617661696c61626c65 = the journal's about information couldn't be read. check that your journal is running, then try again.

m6a6f75726e616c2e636c692e61626f75742e75736167652e6a6f75726e616c = usage: { "solstone journal about [-h | --help]" }

m6a6f75726e616c2e636c692e61626f75742e75736167652e72656d6f7465 = usage: { "solstone about [-h | --help]" }

m6a6f75726e616c2e636c692e6261636b66696c6c2e636f6d6d6974746564 = COMMITTED

m6a6f75726e616c2e636c692e6261636b66696c6c2e6461792e6d697373696e67 = Day { $day } was not found in the journal

m6a6f75726e616c2e636c692e6261636b66696c6c2e64617973 = Could not list chronicle days: { $error }

m6a6f75726e616c2e636c692e6261636b66696c6c2e666c6167732e636f6e666c696374 = usage: { "solstone journal backfill-processing-records [-h] [--day DAY] [--commit |" }{ "\u000a" }{ "                                           --dry-run] [-v] [-d]" }{ "\u000a" }{ "solstone journal backfill-processing-records" }: error: argument { $flag }: not allowed with argument { $previous }

m6a6f75726e616c2e636c692e6261636b66696c6c2e666c6167732e6d697373696e67 = usage: { "solstone journal backfill-processing-records [-h] [--day DAY] [--commit |" }{ "\u000a" }{ "                                           --dry-run] [-v] [-d]" }{ "\u000a" }{ "solstone journal backfill-processing-records" }: error: argument { "--day" }: expected one argument

m6a6f75726e616c2e636c692e6261636b66696c6c2e666c6167732e756e6b6e6f776e = usage: { "solstone journal backfill-processing-records [-h] [--day DAY] [--commit |" }{ "\u000a" }{ "                                           --dry-run] [-v] [-d]" }{ "\u000a" }{ "solstone journal backfill-processing-records" }: error: unrecognized arguments: { $argument }

m6a6f75726e616c2e636c692e6261636b66696c6c2e68656c70 = usage: { "solstone journal backfill-processing-records [-h] [--day DAY] [--commit |" }{ "\u000a" }{ "                                           --dry-run] [-v] [-d]" }{ "\u000a" }{ "\u000a" }Backfill empty processing records onto stuck header-only native{ "\u000a" }{ "describe/transcribe" } outputs{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  " }{ "-h, --help" }{ "     " }show this help message and exit{ "\u000a" }{ "  " }{ "--day DAY" }{ "      " }Day in { "YYYYMMDD" } format; defaults to all days{ "\u000a" }{ "  " }{ "--commit" }{ "       " }Rewrite eligible { "JSONL" } headers with empty processing records{ "\u000a" }{ "  " }{ "--dry-run" }{ "      " }Preview eligible outputs without writing changes{ "\u000a" }{ "  " }{ "-v, --verbose" }{ "  " }Enable verbose output{ "\u000a" }{ "  " }{ "-d, --debug" }{ "    " }Enable debug logging

m6a6f75726e616c2e636c692e6261636b66696c6c2e6964656e746974792e616d626967756f7573 = named stream directory "{ "_default" }" cannot be spelled as a record identity: { $path }

m6a6f75726e616c2e636c692e6261636b66696c6c2e6964656e746974792e6475706c6963617465 = multiple segments share stream { $stream } key { $key }

m6a6f75726e616c2e636c692e6261636b66696c6c2e6964656e746974792e656e636f64696e67 = segment path is not { "UTF-8" } representable: { $path }

m6a6f75726e616c2e636c692e6261636b66696c6c2e696e76616c69642e646179 = expected day in { "YYYYMMDD" } format

m6a6f75726e616c2e636c692e6261636b66696c6c2e6d61726b6572 = Stamped { $path }, but could not mark day { $day } updated: { $error }

m6a6f75726e616c2e636c692e6261636b66696c6c2e70726576696577 = DRY RUN (no changes written)

m6a6f75726e616c2e636c692e6261636b66696c6c2e7365676d656e742e6469726563746f7279 = Could not list segment directory { $path }: { $error }

m6a6f75726e616c2e636c692e6261636b66696c6c2e7365676d656e7473 = Could not list segments for day { $day }: { $error }

m6a6f75726e616c2e636c692e6261636b66696c6c2e7374616d702e6368616e676564 = Could not stamp { $path }: file changed after classification

m6a6f75726e616c2e636c692e6261636b66696c6c2e7374616d702e72656164 = Could not stamp { $path }: could not re-read file: { $error }

m6a6f75726e616c2e636c692e6261636b66696c6c2e7374616d702e7772697465 = Could not stamp { $path }: write failed: { $error }

m6a6f75726e616c2e636c692e6261636b66696c6c2e73747265616d2e6469726563746f7279 = Could not list stream directory { $path }: not a directory

m6a6f75726e616c2e636c692e6261636b66696c6c2e73747265616d2e656e636f64696e67 = segment stream is not { "UTF-8" } representable: { $path }

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e63616e6f6e6963616c2e7265636f766572792e63686172616374657273 = canonical recovery key contains invalid { "Crockford" } characters

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e63616e6f6e6963616c2e7265636f766572792e6c656e677468 = canonical recovery key must be exactly 64 characters

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e656e74726f7079 = could not generate backup key.

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e686f737465642e64656c657465 = could not delete hosted binding.

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e686f737465642e72656164 = could not read hosted binding.

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e686f737465642e7772697465 = could not save hosted binding.

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e696e76616c69642e64657374696e6174696f6e2e7368617065 = backup destination must be a { "JSON" } object

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e696e76616c69642e6d6f6465 = backup mode must be { "byo" } or { "operated" }

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e696e76616c69642e6f66666c6f61642e6279746573 = backup offload byte values must be positive integers or null

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e696e76616c69642e6f66666c6f61642e656e61626c6564 = backup offload enabled must be a boolean

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e696e76616c69642e6f66666c6f61642e7368617065 = backup offload must include { "enabled" }, { "budget_bytes" }, { "floor_bytes" }

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e696e76616c69642e6f66666c6f61642e737461747573 = backup offload status must be { "ok" }, { "skipped" }, { "stalled" }, or { "error" }

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e696e76616c69642e726573746f72652e636f756e74657273 = backup restore counters must be non-negative integers

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e696e76616c69642e726573746f72652e73636f7065 = backup restore scope must be { "day" }, { "all" }, or { "journal" }

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e696e76616c69642e726573746f72652e737461747573 = backup restore status must be { "ok" }, { "no_op" }, { "refused" }, { "degraded" }, or { "error" }

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e696e76616c69642e726574656e74696f6e2e7368617065 = backup retention must include { "hourly" }, { "daily" }, { "weekly" }, { "monthly" }

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e696e76616c69642e726574656e74696f6e2e76616c7565 = backup retention values must be non-negative integers

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e696e76616c69642e766572696669636174696f6e2e737461747573 = backup verification status must be { "ok" }, { "skipped" }, or { "error" }

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e6c6f636b2e74696d656f7574 = could not acquire lock for { $path } within { $seconds }s

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e6d697373696e672e63726564656e7469616c = missing backup credential: { $key }

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e7265636f766572792e7061727365 = recovery key must contain exactly 64 { "Crockford" } characters after cleanup

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e73746f7265642e6b657973 = backup keys must be strings when present

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e74656172646f776e2e7265636f766572792e636f7272757074 = teardown recovery record corrupt or invalid.

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e74656172646f776e2e7265636f766572792e64656c657465 = could not delete teardown recovery record.

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e74656172646f776e2e7265636f766572792e72656164 = could not read teardown recovery record.

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e74656172646f776e2e7265636f766572792e7772697465 = could not save teardown recovery record.

m6a6f75726e616c2e636c692e6261636b75702e636f6e6669672e756e737570706f727465642e6261636b656e64 = unsupported backup backend

m6a6f75726e616c2e636c692e6261636b75702e64657374696e6174696f6e2e6261636b656e642e707265666978 = The repository must start with { $backend }: (for example { $example }).

m6a6f75726e616c2e636c692e6261636b75702e64657374696e6174696f6e2e686f73746564 = { "sol pbc" } runs your backup storage, so there's no destination to change here. manage it in the services portal.

m6a6f75726e616c2e636c692e6261636b75702e64657374696e6174696f6e2e726566757365642e617574682e6661696c6564 = the destination rejected the credentials. nothing was saved.

m6a6f75726e616c2e636c692e6261636b75702e64657374696e6174696f6e2e726566757365642e6275636b65742e6e6f742e666f756e64 = these credentials can't open a bucket by that name. check the bucket name in the repository. nothing was saved.

m6a6f75726e616c2e636c692e6261636b75702e64657374696e6174696f6e2e726566757365642e6c6f636b6564 = the backup there is busy. try again shortly. nothing was saved.

m6a6f75726e616c2e636c692e6261636b75702e64657374696e6174696f6e2e726566757365642e6f74686572 = the destination couldn't be opened. check the repository and credentials, then try again. nothing was saved.

m6a6f75726e616c2e636c692e6261636b75702e64657374696e6174696f6e2e726566757365642e7265706f2e6d697373696e67 = no backup was found there with these credentials. nothing was saved.

m6a6f75726e616c2e636c692e6261636b75702e64657374696e6174696f6e2e726566757365642e7265706f7369746f72792e6b65792e72656675736564 = there's already a backup there that your recovery key can't open. choose another destination. nothing was saved.

m6a6f75726e616c2e636c692e6261636b75702e64657374696e6174696f6e2e726566757365642e74696d656f7574 = the destination didn't answer in time. check the repository and credentials, then try again. nothing was saved.

m6a6f75726e616c2e636c692e6261636b75702e64657374696e6174696f6e2e73657475702e6661696c6564 = a backup couldn't be set up there ({ $reason }). nothing was saved.

m6a6f75726e616c2e636c692e6261636b75702e64657374696e6174696f6e2e7377697463682e636f6e6669726d = to switch, run again with { "--confirm-new-repository" }.

m6a6f75726e616c2e636c692e6261636b75702e64657374696e6174696f6e2e7377697463682e6c656164 = that's a different repository. your existing backup stays where it is, untouched, and your journal won't back up to it or restore from it.

m6a6f75726e616c2e636c692e6261636b75702e656e61626c652e636f6d706c657465 = Backup enabled.

m6a6f75726e616c2e636c692e6261636b75702e656e61626c652e636f6d706c6574652e6b6579 = Backup enabled. Your recovery key:{ "\u000a" }{ $key_grid }

m6a6f75726e616c2e636c692e6261636b75702e656e61626c652e636f6e6669726d2e6b6579 = Your recovery key (write it down - it is the only way to restore):{ "\u000a" }{ $key_grid }{ "\u000a" }Confirm by piping the key back: { "solstone journal backup enable" }

m6a6f75726e616c2e636c692e6261636b75702e656e61626c652e64657374696e6174696f6e2e7265717569726564 = Set a destination first: { "solstone journal backup destination set" }

m6a6f75726e616c2e636c692e6261636b75702e656e61626c652e6b65792e6d69736d61746368 = Recovery key did not match.

m6a6f75726e616c2e636c692e6261636b75702e656e61626c652e7265706f7369746f72792e6d697373696e67 = Repository not found; pre-initialize it or set { "backup.recovery_key" } in config.

m6a6f75726e616c2e636c692e6261636b75702e6572726f72 = Error: { $message }

m6a6f75726e616c2e636c692e6261636b75702e68656c702e64657374696e6174696f6e = usage: { "solstone journal backup destination <command> [options]" }

m6a6f75726e616c2e636c692e6261636b75702e68656c702e6f66666c6f6164 = usage: { "solstone journal backup offload <command> [options]" }

m6a6f75726e616c2e636c692e6261636b75702e68656c702e7265636f766572792e6b6579 = usage: { "solstone journal backup recovery-key <command> [options]" }

m6a6f75726e616c2e636c692e6261636b75702e68656c702e726f6f74 = usage: { "solstone journal backup <command> [options]" }

m6a6f75726e616c2e636c692e6261636b75702e686f737465642e6669656c64 = Missing { $name }.

m6a6f75726e616c2e636c692e6261636b75702e696e69742e636f6465 = Repository initialization failed ({ "code=" }{ $returncode }).

m6a6f75726e616c2e636c692e6261636b75702e696e69742e6661696c6564 = repository initialization failed.

m6a6f75726e616c2e636c692e6261636b75702e696e69742e72657461696e6564 = repository initialization failed; retained operation state requires review.

m6a6f75726e616c2e636c692e6261636b75702e696e7075742e696f = { "stdin" } I/O error.

m6a6f75726e616c2e636c692e6261636b75702e696e7075742e6a736f6e = invalid { "JSON" } on { "stdin" }: { $error }

m6a6f75726e616c2e636c692e6261636b75702e696e7075742e6c696d6974 = { "stdin" } request exceeds the { "JSON" } input limit.

m6a6f75726e616c2e636c692e6261636b75702e696e7075742e6f626a656374 = expected { "JSON" } object on { "stdin" }.

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e6d656469612e627974652e6f766572666c6f77 = media byte total overflow

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e6d656469612e66696c652e6f766572666c6f77 = media file total overflow

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e707265766965772e68656c64 = a read-only offload preview is unavailable. no offload work was started.

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e6461792e696e76616c6964 = Invalid day.

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e6461792e6f722e616c6c = Use either a day or { "--all" }, not both.

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e6461792e7265717569726564 = Provide a day or { "--all" }.

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e68656164696e67 = backup offload restore:

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e617263686976652e756e626f756e64 = the backup record doesn't identify the archive needed to restore these files

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e6261636b75702e6e6f742e7265616479 = your backup isn't set up, so the offloaded files can't be fetched back

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e64657374696e6174696f6e2e61646d697373696f6e2e6661696c6564 = the backup destination couldn't be reached

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e6661696c6564 = the restore attempt failed for an unspecified reason

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e696e73756666696369656e742e667265652e7370616365 = there isn't enough free space on this computer to bring the files back

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e6c65646765722e6465677261646564 = the record of what was offloaded is damaged and can't be used to restore

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e6c65676163792e726561642e6661696c6564 = the backup copy of media offloaded by an earlier version of your journal couldn't be read, so it wasn't restored

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e6c65676163792e7265766965772e68656c64 = media offloaded by an earlier version of your journal couldn't be checked against your backup, so it wasn't restored

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e6c65676163792e766572696669636174696f6e2e6661696c6564 = the backup copy of media offloaded by an earlier version of your journal didn't match its record, so it wasn't restored

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e6c6f636b6564 = the backup at the destination is locked, usually by a run that's still going or one that was interrupted

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e6d697373696e672e66696c652e61667465722e726573746f7265 = a file that should have come back with the restore is missing

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e6e6f7468696e672e746f2e726573746f7265 = there was nothing offloaded to restore

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e72636c6f6e652e756e617661696c61626c65 = the { "rclone" } program isn't available

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e7265706f2e6d697373696e67 = there's no backup at the destination yet

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e7265737469632e756e617661696c61626c65 = the { "restic" } backup program isn't available

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e726573746f72652e636f6e666c696374 = a different file is already where at least one of these files belongs, so the file already there was kept and the backup copy wasn't restored

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e726573746f72652e68656c64 = restoring offloaded files is temporarily unavailable

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e726573746f72652e6e6f742e7075626c6973686564 = at least one file couldn't be put back in your journal this time and nothing was put in its place, so try again later

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e7365676d656e742e6964656e74697479 = one of your journal's recordings couldn't be verified against its own identity record

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e7365676d656e742e6d697373696e67 = the journal location these files belong to no longer exists on this computer

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e73747265616d2e6d61726b65722e6661696c6564 = the files were restored, but updating the journal's records afterward failed

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e74696d656f7574 = it ran too long and stopped

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e756e6b6e6f776e2e636f6465 = stopped without a reason ({ $code })

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e726561736f6e2e766572696669636174696f6e2e6661696c6564 = a restored file didn't match what it should have been, so it may be damaged

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e726573746f72652e756e66696e6973686564 = the restore didn't finish cleanly, so try again later.

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e7374617475732e68656164696e67 = backup offload:

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e7374617475732e696e76616c6964 = invalid offload status.

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e7374617475732e6f766572666c6f77 = media status total overflow

m6a6f75726e616c2e636c692e6261636b75702e6f66666c6f61642e7573652e696e7374616c6c6564 = run `{ "solstone journal backup offload run" }` with the installed { "solstone" } command.

m6a6f75726e616c2e636c692e6261636b75702e6f75747075742e6a736f6e = could not render { "JSON" } output.

m6a6f75726e616c2e636c692e6261636b75702e70617573652e616c7265616479 = backup is already paused.

m6a6f75726e616c2e636c692e6261636b75702e70617573652e636f6d706c657465 = backup paused. run `{ "solstone journal backup resume" }` to start it again.

m6a6f75726e616c2e636c692e6261636b75702e70617573652e6f6666 = backup is off, so there's nothing to pause.

m6a6f75726e616c2e636c692e6261636b75702e7061796c6f61642e6261636b656e64 = Missing backend.

m6a6f75726e616c2e636c692e6261636b75702e7061796c6f61642e63726564656e7469616c = Missing credential: { $key }.

m6a6f75726e616c2e636c692e6261636b75702e7061796c6f61642e63726564656e7469616c73 = Missing credentials.

m6a6f75726e616c2e636c692e6261636b75702e7061796c6f61642e7265636f766572792e6b6579 = Missing { "recovery_key" }.

m6a6f75726e616c2e636c692e6261636b75702e7061796c6f61642e7265706f7369746f7279 = Missing repository.

m6a6f75726e616c2e636c692e6261636b75702e7061796c6f61642e756e737570706f72746564 = Unsupported backend.

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e636f6d706c657465 = Retention prune complete.

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e64657461696c2e6661696c7572652e72636c6f6e652e756e617661696c61626c65 = Prune failed: the { "rclone" } program isn't available. { "detail=" }{ $detail }

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e64657461696c2e6661696c7572652e7265737469632e756e617661696c61626c65 = Prune failed: the { "restic" } backup program isn't available. { "detail=" }{ $detail }

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e6661696c7572652e616363657373 = Prune failed: the destination refused permission for this write.

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e6661696c7572652e617574682e6661696c6564 = Prune failed: the destination rejected the credentials.

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e6661696c7572652e62696e64696e672e696e76616c6964 = Prune failed: your journal's backup settings are no longer valid.

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e6661696c7572652e62696e64696e672e73757065727365646564 = Prune failed: newer backup settings replaced these.

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e6661696c7572652e62726f6b65722e6572726f72 = Prune failed: the backup service { "sol pbc" } runs couldn't complete the request.

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e6661696c7572652e62726f6b65722e756e726561636861626c65 = Prune failed: the backup service { "sol pbc" } runs couldn't be reached.

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e6661696c7572652e6275636b65742e6e6f742e666f756e64 = Prune failed: your credentials can't open the bucket the repository names.

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e6661696c7572652e6661696c6564 = Prune failed: no reason was given.

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e6661696c7572652e686f737465642e656e7469746c656d656e742e696e616374697665 = Prune failed: { "sol pbc" } isn't running backup for you.

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e6661696c7572652e6a6f75726e616c2e706174682e756e7265736f6c766564 = Prune failed: couldn't find your journal.

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e6661696c7572652e6c6f636b6564 = Prune failed: the backup at the destination is locked, usually by a run that's still going or one that was interrupted.

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e6661696c7572652e72636c6f6e652e756e617661696c61626c65 = Prune failed: the { "rclone" } program isn't available.

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e6661696c7572652e7265706f2e657869737473 = Prune failed: there's already a backup there.

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e6661696c7572652e7265706f2e6d697373696e67 = Prune failed: there's no backup at the destination yet.

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e6661696c7572652e7265706f7369746f72792e756e737570706f72746564 = Prune failed: the repository in your backup settings doesn't start with { "s3:" } or { "b2:" } to match its backend.

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e6661696c7572652e7265737469632e756e617661696c61626c65 = Prune failed: the { "restic" } backup program isn't available.

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e6661696c7572652e74696d656f7574 = Prune failed: it ran too long and stopped.

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e6661696c7572652e756e6b6e6f776e2e636f6465 = Prune failed: stopped without a reason ({ $code }).

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e6661696c7572652e756e726561636861626c65 = Prune failed: the destination couldn't be reached.

m6a6f75726e616c2e636c692e6261636b75702e7072756e652e736b6970706564 = Prune skipped (not enabled or not configured).

m6a6f75726e616c2e636c692e6261636b75702e7265636f766572792e6b65792e6d697373696e67 = No recovery key is set.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e616c72656164792e7365742e7570 = restore couldn't start: your journal is already set up, so it can't be restored over. to use a backup, start a new journal and choose restore during setup.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e636f6d706c657465 = restore complete: { $counters }, { "integrity_ok=" }{ $integrity }, { "resumable=" }{ $resumable }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e64656772616465642e696e746567726974792e6661696c6564 = restore finished with a problem: { $counters }; the backup failed its integrity check and may be damaged, so your journal isn't set to the backup ({ "reason_code=" }{ $code }).

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e64656772616465642e696e746567726974792e756e7665726966696564 = restore finished with a problem: { $counters }; the integrity check couldn't run (the backup was busy or timed out), so your journal isn't set to the backup ({ "reason_code=" }{ $code }).

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e64656772616465642e6f74686572 = restore finished with a problem: { $counters }; restore completed with degraded evidence ({ "reason_code=" }{ $code }).

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e64656772616465642e726573746f72652e73756d6d6172792e6d697373696e67 = restore finished with a problem: { $counters }; restore summary was missing or malformed ({ "reason_code=" }{ $code }).

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c65642e636f756e74657273 = restore failed: { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e626f64792e72656275696c642e6661696c6564 = restore failed: your journal couldn't be rebuilt from the restored files; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e64657374696e6174696f6e2e61646d697373696f6e2e6661696c6564 = restore failed: the backup destination couldn't be reached; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e64657374696e6174696f6e2e696e76616c6964 = restore failed: the backup destination isn't valid; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e64657374696e6174696f6e2e7075626c6973682e6661696c6564 = restore failed: the restored journal couldn't be saved to its destination; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e66756c6c2e7363616e2e6661696c6564 = restore failed: a full scan of the restored journal failed; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e696e73756666696369656e742e667265652e7370616365 = restore failed: there isn't enough free space for this restore, so nothing changed; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e696e746567726974792e636865636b2e696f2e6661696c6564 = restore failed: the restored files couldn't be checked; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e696e746567726974792e6661696c6564 = restore failed: the backup failed its integrity check and may be damaged, so your journal isn't set to the backup; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e696e746567726974792e756e7665726966696564 = restore failed: the integrity check couldn't run (the backup was busy or timed out), so your journal isn't set to the backup; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e696e76616c69642e6b6579 = restore failed: the recovery key entered doesn't match this backup; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e6a6f75726e616c2e616c72656164792e7365742e7570 = restore failed: your journal is already set up, so it can't be restored over. to use a backup, start a new journal and choose restore during setup; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e6a6f75726e616c2e736e617073686f742e6e6f742e666f756e64 = restore failed: no backup of your journal was found; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e7265636f766572792e636f6e6669726d6174696f6e2e7075626c6973682e6661696c6564 = restore failed: the recovery confirmation couldn't be saved; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e7265636f766572792e6b65792e7075626c6973682e6661696c6564 = restore failed: the recovery key couldn't be saved; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e7265737469632e756e617661696c61626c65 = restore failed: the { "restic" } backup program isn't available; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e726573746f72652e6661696c6564 = restore failed: the restore didn't complete; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e726573746f72652e696f2e6661696c6564 = restore failed: files couldn't be written during restore; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e726573746f72652e7265636f72642e6661696c6564 = restore failed: the restore's result couldn't be recorded; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e726573746f72652e73756d6d6172792e6d697373696e67 = restore failed: the restore finished but its summary is missing; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e736e617073686f742e636174616c6f672e696e76616c6964 = restore failed: the list of backups is invalid; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e736e617073686f742e6c6973742e6661696c6564 = restore failed: the list of backups couldn't be retrieved; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e736e617073686f742e6c6973742e696f2e6661696c6564 = restore failed: the list of backups couldn't be read; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e736e617073686f742e73656c656374696f6e2e616d626967756f7573 = restore failed: more than one backup matched and none could be chosen automatically; { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e6661696c7572652e756e6b6e6f776e2e636f6465 = restore failed: stopped without a reason ({ $code }); { $counters }.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e626f64792e72656275696c642e6661696c6564 = Recording the result also failed: your journal couldn't be rebuilt from the restored files.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e64657374696e6174696f6e2e61646d697373696f6e2e6661696c6564 = Recording the result also failed: the backup destination couldn't be reached.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e64657374696e6174696f6e2e696e76616c6964 = Recording the result also failed: the backup destination isn't valid.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e64657374696e6174696f6e2e7075626c6973682e6661696c6564 = Recording the result also failed: the restored journal couldn't be saved to its destination.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e66756c6c2e7363616e2e6661696c6564 = Recording the result also failed: a full scan of the restored journal failed.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e696e73756666696369656e742e667265652e7370616365 = Recording the result also failed: there isn't enough free space for this restore, so nothing changed.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e696e746567726974792e636865636b2e696f2e6661696c6564 = Recording the result also failed: the restored files couldn't be checked.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e696e746567726974792e6661696c6564 = Recording the result also failed: the backup failed its integrity check and may be damaged, so your journal isn't set to the backup.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e696e746567726974792e756e7665726966696564 = Recording the result also failed: the integrity check couldn't run (the backup was busy or timed out), so your journal isn't set to the backup.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e696e76616c69642e6b6579 = Recording the result also failed: the recovery key entered doesn't match this backup.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e6a6f75726e616c2e616c72656164792e7365742e7570 = Recording the result also failed: your journal is already set up, so it can't be restored over. to use a backup, start a new journal and choose restore during setup.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e6a6f75726e616c2e736e617073686f742e6e6f742e666f756e64 = Recording the result also failed: no backup of your journal was found.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e7265636f766572792e636f6e6669726d6174696f6e2e7075626c6973682e6661696c6564 = Recording the result also failed: the recovery confirmation couldn't be saved.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e7265636f766572792e6b65792e7075626c6973682e6661696c6564 = Recording the result also failed: the recovery key couldn't be saved.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e7265737469632e756e617661696c61626c65 = Recording the result also failed: the { "restic" } backup program isn't available.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e726573746f72652e6661696c6564 = Recording the result also failed: the restore didn't complete.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e726573746f72652e696f2e6661696c6564 = Recording the result also failed: files couldn't be written during restore.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e726573746f72652e7265636f72642e6661696c6564 = Recording the result also failed: the restore's result couldn't be recorded.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e726573746f72652e73756d6d6172792e6d697373696e67 = Recording the result also failed: the restore finished but its summary is missing.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e736e617073686f742e636174616c6f672e696e76616c6964 = Recording the result also failed: the list of backups is invalid.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e736e617073686f742e6c6973742e6661696c6564 = Recording the result also failed: the list of backups couldn't be retrieved.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e736e617073686f742e6c6973742e696f2e6661696c6564 = Recording the result also failed: the list of backups couldn't be read.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e736e617073686f742e73656c656374696f6e2e616d626967756f7573 = Recording the result also failed: more than one backup matched and none could be chosen automatically.

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e7265636f7264696e672e6661696c7572652e756e6b6e6f776e2e636f6465 = Recording the result also failed: stopped without a reason ({ $code }).

m6a6f75726e616c2e636c692e6261636b75702e726573746f72652e756e7265636f72646564 = restore completed, but recording the result failed: { $counters } ({ "reason_code=" }{ "restore_record_failed" }).

m6a6f75726e616c2e636c692e6261636b75702e726573756d652e636f6d706c657465 = backup resumed. the next backup runs on its usual schedule.

m6a6f75726e616c2e636c692e6261636b75702e726573756d652e6e6f742e706175736564 = backup isn't paused.

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e6661696c6564 = Rotation failed: failed.

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e6661696c7572652e616363657373 = Rotation failed: the destination refused permission for this write.

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e6661696c7572652e617574682e6661696c6564 = Rotation failed: the destination rejected the credentials.

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e6661696c7572652e62696e64696e672e696e76616c6964 = Rotation failed: your journal's backup settings are no longer valid.

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e6661696c7572652e62696e64696e672e73757065727365646564 = Rotation failed: newer backup settings replaced these.

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e6661696c7572652e62726f6b65722e6572726f72 = Rotation failed: the backup service { "sol pbc" } runs couldn't complete the request.

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e6661696c7572652e62726f6b65722e756e726561636861626c65 = Rotation failed: the backup service { "sol pbc" } runs couldn't be reached.

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e6661696c7572652e6275636b65742e6e6f742e666f756e64 = Rotation failed: your credentials can't open the bucket the repository names.

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e6661696c7572652e6661696c6564 = Rotation failed: no reason was given.

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e6661696c7572652e686f737465642e656e7469746c656d656e742e696e616374697665 = Rotation failed: { "sol pbc" } isn't running backup for you.

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e6661696c7572652e6a6f75726e616c2e706174682e756e7265736f6c766564 = Rotation failed: couldn't find your journal.

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e6661696c7572652e6c6f636b6564 = Rotation failed: the backup at the destination is locked, usually by a run that's still going or one that was interrupted.

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e6661696c7572652e72636c6f6e652e756e617661696c61626c65 = Rotation failed: the { "rclone" } program isn't available.

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e6661696c7572652e7265706f2e657869737473 = Rotation failed: there's already a backup there.

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e6661696c7572652e7265706f2e6d697373696e67 = Rotation failed: there's no backup at the destination yet.

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e6661696c7572652e7265706f7369746f72792e756e737570706f72746564 = Rotation failed: the repository in your backup settings doesn't start with { "s3:" } or { "b2:" } to match its backend.

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e6661696c7572652e7265737469632e756e617661696c61626c65 = Rotation failed: the { "restic" } backup program isn't available.

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e6661696c7572652e74696d656f7574 = Rotation failed: it ran too long and stopped.

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e6661696c7572652e756e6b6e6f776e2e636f6465 = Rotation failed: stopped without a reason ({ $code }).

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e6661696c7572652e756e726561636861626c65 = Rotation failed: the destination couldn't be reached.

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e6b6579 = New recovery key (write it down):{ "\u000a" }{ $key_grid }

m6a6f75726e616c2e636c692e6261636b75702e726f746174696f6e2e736b6970706564 = Recovery key rotation skipped (backup not configured).

m6a6f75726e616c2e636c692e6261636b75702e72756e2e636f6d706c657465 = Backup complete (snapshot { $snapshot }).

m6a6f75726e616c2e636c692e6261636b75702e72756e2e6661696c7572652e616363657373 = Backup failed: the destination refused permission for this write.

m6a6f75726e616c2e636c692e6261636b75702e72756e2e6661696c7572652e617574682e6661696c6564 = Backup failed: the destination rejected the credentials.

m6a6f75726e616c2e636c692e6261636b75702e72756e2e6661696c7572652e62696e64696e672e696e76616c6964 = Backup failed: your journal's backup settings are no longer valid.

m6a6f75726e616c2e636c692e6261636b75702e72756e2e6661696c7572652e62696e64696e672e73757065727365646564 = Backup failed: newer backup settings replaced these.

m6a6f75726e616c2e636c692e6261636b75702e72756e2e6661696c7572652e62726f6b65722e6572726f72 = Backup failed: the backup service { "sol pbc" } runs couldn't complete the request.

m6a6f75726e616c2e636c692e6261636b75702e72756e2e6661696c7572652e62726f6b65722e756e726561636861626c65 = Backup failed: the backup service { "sol pbc" } runs couldn't be reached.

m6a6f75726e616c2e636c692e6261636b75702e72756e2e6661696c7572652e6275636b65742e6e6f742e666f756e64 = Backup failed: your credentials can't open the bucket the repository names.

m6a6f75726e616c2e636c692e6261636b75702e72756e2e6661696c7572652e6661696c6564 = Backup failed: no reason was given.

m6a6f75726e616c2e636c692e6261636b75702e72756e2e6661696c7572652e686f737465642e656e7469746c656d656e742e696e616374697665 = Backup failed: { "sol pbc" } isn't running backup for you.

m6a6f75726e616c2e636c692e6261636b75702e72756e2e6661696c7572652e6a6f75726e616c2e706174682e756e7265736f6c766564 = Backup failed: couldn't find your journal.

m6a6f75726e616c2e636c692e6261636b75702e72756e2e6661696c7572652e6c6f636b6564 = Backup failed: the backup at the destination is locked, usually by a run that's still going or one that was interrupted.

m6a6f75726e616c2e636c692e6261636b75702e72756e2e6661696c7572652e72636c6f6e652e756e617661696c61626c65 = Backup failed: the { "rclone" } program isn't available.

m6a6f75726e616c2e636c692e6261636b75702e72756e2e6661696c7572652e7265706f2e657869737473 = Backup failed: there's already a backup there.

m6a6f75726e616c2e636c692e6261636b75702e72756e2e6661696c7572652e7265706f2e6d697373696e67 = Backup failed: there's no backup at the destination yet.

m6a6f75726e616c2e636c692e6261636b75702e72756e2e6661696c7572652e7265706f7369746f72792e756e737570706f72746564 = Backup failed: the repository in your backup settings doesn't start with { "s3:" } or { "b2:" } to match its backend.

m6a6f75726e616c2e636c692e6261636b75702e72756e2e6661696c7572652e7265737469632e756e617661696c61626c65 = Backup failed: the { "restic" } backup program isn't available.

m6a6f75726e616c2e636c692e6261636b75702e72756e2e6661696c7572652e74696d656f7574 = Backup failed: it ran too long and stopped.

m6a6f75726e616c2e636c692e6261636b75702e72756e2e6661696c7572652e756e6b6e6f776e2e636f6465 = Backup failed: stopped without a reason ({ $code }).

m6a6f75726e616c2e636c692e6261636b75702e72756e2e6661696c7572652e756e726561636861626c65 = Backup failed: the destination couldn't be reached.

m6a6f75726e616c2e636c692e6261636b75702e72756e2e7061727469616c2e736e617073686f742e636f756e74 = { $i18nCategory ->
    [one] Backup partial (snapshot { $snapshot }). 1 file couldn't be read, so it isn't in it. Run `{ "solstone journal backup run" }` again once that's fixed.
   *[other] Backup partial (snapshot { $snapshot }). { $count } files couldn't be read, so they aren't in it. Run `{ "solstone journal backup run" }` again once that's fixed.
}

m6a6f75726e616c2e636c692e6261636b75702e72756e2e7061727469616c2e736e617073686f742e636f756e742e726561736f6e = { $i18nCategory ->
    [one] Backup partial (snapshot { $snapshot }). 1 file couldn't be read, so it isn't in it. The first: { $reason }. Run `{ "solstone journal backup run" }` again once that's fixed.
   *[other] Backup partial (snapshot { $snapshot }). { $count } files couldn't be read, so they aren't in it. The first: { $reason }. Run `{ "solstone journal backup run" }` again once that's fixed.
}

m6a6f75726e616c2e636c692e6261636b75702e72756e2e7061727469616c2e736e617073686f742e756e6d65617375726564 = Backup partial (snapshot { $snapshot }). some files couldn't be read, so they aren't in it. Run `{ "solstone journal backup run" }` again once that's fixed.

m6a6f75726e616c2e636c692e6261636b75702e72756e2e7061727469616c2e756e636f6e6669726d65642e636f756e74 = { $i18nCategory ->
    [one] Backup partial. 1 file couldn't be read, so it isn't in it, and the snapshot couldn't be confirmed. Run `{ "solstone journal backup run" }` again once that's fixed.
   *[other] Backup partial. { $count } files couldn't be read, so they aren't in it, and the snapshot couldn't be confirmed. Run `{ "solstone journal backup run" }` again once that's fixed.
}

m6a6f75726e616c2e636c692e6261636b75702e72756e2e7061727469616c2e756e636f6e6669726d65642e636f756e742e726561736f6e = { $i18nCategory ->
    [one] Backup partial. 1 file couldn't be read, so it isn't in it, and the snapshot couldn't be confirmed. The first: { $reason }. Run `{ "solstone journal backup run" }` again once that's fixed.
   *[other] Backup partial. { $count } files couldn't be read, so they aren't in it, and the snapshot couldn't be confirmed. The first: { $reason }. Run `{ "solstone journal backup run" }` again once that's fixed.
}

m6a6f75726e616c2e636c692e6261636b75702e72756e2e7061727469616c2e756e636f6e6669726d65642e756e6d65617375726564 = Backup partial. some files couldn't be read, so they aren't in it, and the snapshot couldn't be confirmed. Run `{ "solstone journal backup run" }` again once that's fixed.

m6a6f75726e616c2e636c692e6261636b75702e72756e2e736b6970706564 = Backup skipped (not enabled or not configured).

m6a6f75726e616c2e636c692e6261636b75702e74656172646f776e2e6261636b75702e6f6e6c792e68656c64 = your backup holds the only copy of some of your media, so no backup changes were made. restore everything from the backup page first, then try again.

m6a6f75726e616c2e636c692e6261636b75702e74656172646f776e2e636f6e6669726d = Refusing to tear down backup without { "--yes" }. This forgets all snapshots.

m6a6f75726e616c2e636c692e6261636b75702e74656172646f776e2e6e6f742e73746172746564 = turning off and deleting your backup couldn't start. no backup changes were made.

m6a6f75726e616c2e636c692e6261636b75702e74656172646f776e2e7061727469616c = backup teardown did not finish. keep recovery files and review the recorded effects before retrying.

m6a6f75726e616c2e636c692e6261636b75702e74656172646f776e2e7072656c61756e63682e68656c64 = turning off and deleting your backup isn't available here. no backup changes were made.

m6a6f75726e616c2e636c692e6261636b75702e75736167652e64657374696e6174696f6e = { "solstone journal backup destination" }: error: unrecognized arguments: { $arguments }

m6a6f75726e616c2e636c692e6261636b75702e75736167652e6f66666c6f6164 = { "solstone journal backup offload" }: error: unrecognized arguments: { $arguments }

m6a6f75726e616c2e636c692e6261636b75702e75736167652e7265636f766572792e6b6579 = { "solstone journal backup recovery-key" }: error: unrecognized arguments: { $arguments }

m6a6f75726e616c2e636c692e6261636b75702e75736167652e726f6f74 = { "solstone journal backup" }: error: unrecognized arguments: { $arguments }

m6a6f75726e616c2e636c692e627261696e2e617267756d656e74732e696e76616c6964 = { "solstone journal brain" }: error: invalid arguments

m6a6f75726e616c2e636c692e627261696e2e62757379 = Thinking busy: a check is already running

m6a6f75726e616c2e636c692e627261696e2e636865636b2e636f6e666967 = thinking check failed: could not read journal config: { $detail }

m6a6f75726e616c2e636c692e627261696e2e636865636b2e6661696c757265 = thinking check failed: { $detail }

m6a6f75726e616c2e636c692e627261696e2e636865636b2e66696e6765727072696e742e6f7074696f6e73 = expected active fingerprint and expected absence are mutually exclusive

m6a6f75726e616c2e636c692e627261696e2e636865636b2e6b65792e6d697373696e67 = the provider needs a key, and your journal couldn't confirm the result was saved.

m6a6f75726e616c2e636c692e627261696e2e636865636b2e6b65792e72656a6563746564 = the provider rejected this key, and your journal couldn't confirm the result was saved.

m6a6f75726e616c2e636c692e627261696e2e636865636b2e6f70656e2e6469726563746f7279 = open flat-directory descendant failed for { $path }: { $reason }

m6a6f75726e616c2e636c692e627261696e2e636865636b2e72616e646f6d = could not generate secure random bytes

m6a6f75726e616c2e636c692e627261696e2e68656164696e672e626c6f636b6564 = Thinking blocked: { $reason }

m6a6f75726e616c2e636c692e627261696e2e68656164696e672e626c6f636b65642e636f6d706f6e656e74 = Thinking blocked: { $reason } ({ $component })

m6a6f75726e616c2e636c692e627261696e2e68656164696e672e636865636b696e67 = Thinking checking: { $reason }

m6a6f75726e616c2e636c692e627261696e2e68656164696e672e636865636b696e672e636f6d706f6e656e74 = Thinking checking: { $reason } ({ $component })

m6a6f75726e616c2e636c692e627261696e2e68656164696e672e6f74686572 = Thinking { $state }: { $reason }

m6a6f75726e616c2e636c692e627261696e2e68656164696e672e6f746865722e636f6d706f6e656e74 = Thinking { $state }: { $reason } ({ $component })

m6a6f75726e616c2e636c692e627261696e2e68656164696e672e756e6865616c746879 = Thinking unhealthy: { $reason }

m6a6f75726e616c2e636c692e627261696e2e68656164696e672e756e6865616c7468792e636f6d706f6e656e74 = Thinking unhealthy: { $reason } ({ $component })

m6a6f75726e616c2e636c692e627261696e2e68656164696e672e756e6b6e6f776e = Thinking unknown: { $reason }

m6a6f75726e616c2e636c692e627261696e2e68656164696e672e756e6b6e6f776e2e636f6d706f6e656e74 = Thinking unknown: { $reason } ({ $component })

m6a6f75726e616c2e636c692e627261696e2e68656c702e72656672657368 = usage: { "solstone journal brain refresh [-h] [--json] [--expected-fingerprint EXPECTED_FINGERPRINT]" }{ "\u000a" }{ "                            [--expected-active-fingerprint]" }{ "\u000a" }{ "                            [--expect-active-fingerprint-absent]" }{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  " }{ "-h, --help" }{ "            " }show this help message and exit{ "\u000a" }{ "  " }{ "--json" }{ "                " }Emit { "JSON" } instead of plain output{ "\u000a" }{ "  --expected-fingerprint EXPECTED_FINGERPRINT" }{ "\u000a" }{ "  --expected-active-fingerprint" }{ "\u000a" }{ "  --expect-active-fingerprint-absent" }

m6a6f75726e616c2e636c692e627261696e2e68656c702e72656e6577 = usage: { "solstone journal brain renew-prerequisites [-h]" }{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  " }{ "-h, --help" }{ "  " }show this help message and exit

m6a6f75726e616c2e636c692e627261696e2e68656c702e726f6f74 = usage: { "solstone journal brain [-h] {status,refresh} ..." }{ "\u000a" }{ "\u000a" }Active-brain status and bounded refresh CLI.{ "\u000a" }{ "\u000a" }positional arguments:{ "\u000a" }{ "  {status,refresh}" }{ "\u000a" }    { "status              " }Show active-brain status{ "\u000a" }    { "refresh             " }Run one bounded active-brain check{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  " }{ "-h, --help" }{ "          " }show this help message and exit

m6a6f75726e616c2e636c692e627261696e2e68656c702e737461747573 = usage: { "solstone journal brain status [-h] [--json]" }{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  " }{ "-h, --help" }{ "  " }show this help message and exit{ "\u000a" }{ "  " }{ "--json" }{ "      " }Emit { "JSON" } instead of plain output

m6a6f75726e616c2e636c692e627261696e2e686f6c642e6361706163697479 = credential hold capacity is unavailable

m6a6f75726e616c2e636c692e627261696e2e686f6c642e696e76616c69642e70726f7669646572 = unsupported credential hold provider

m6a6f75726e616c2e636c692e627261696e2e686f6c642e726f6f742e6368616e676564 = admitted journal root changed

m6a6f75726e616c2e636c692e627261696e2e686f6c642e73757065727365646564 = credential validation was superseded

m6a6f75726e616c2e636c692e627261696e2e686f6c642e756e617661696c61626c65 = credential hold state is unavailable

m6a6f75726e616c2e636c692e627261696e2e72656164792e62796f = Thinking ready: your own model

m6a6f75726e616c2e636c692e627261696e2e72656164792e62796f2e636865636b6564 = Thinking ready: your own model, checked { $age } ago

m6a6f75726e616c2e636c692e627261696e2e72656164792e62796f2e6d6f64656c = Thinking ready: your own model { $model }

m6a6f75726e616c2e636c692e627261696e2e72656164792e62796f2e6d6f64656c2e636865636b6564 = Thinking ready: your own model { $model }, checked { $age } ago

m6a6f75726e616c2e636c692e627261696e2e72656164792e6c6f63616c = Thinking ready: local thinking

m6a6f75726e616c2e636c692e627261696e2e72656164792e6c6f63616c2e636865636b6564 = Thinking ready: local thinking, checked { $age } ago

m6a6f75726e616c2e636c692e627261696e2e72656164792e6c6f63616c2e6d6f64656c = Thinking ready: local thinking { $model }

m6a6f75726e616c2e636c692e627261696e2e72656164792e6c6f63616c2e6d6f64656c2e636865636b6564 = Thinking ready: local thinking { $model }, checked { $age } ago

m6a6f75726e616c2e636c692e627261696e2e72656164792e6f74686572 = Thinking ready: { $lane }

m6a6f75726e616c2e636c692e627261696e2e72656164792e6f746865722e636865636b6564 = Thinking ready: { $lane }, checked { $age } ago

m6a6f75726e616c2e636c692e627261696e2e72656164792e6f746865722e6d6f64656c = Thinking ready: { $lane } { $model }

m6a6f75726e616c2e636c692e627261696e2e72656164792e6f746865722e6d6f64656c2e636865636b6564 = Thinking ready: { $lane } { $model }, checked { $age } ago

m6a6f75726e616c2e636c692e627261696e2e72656164792e737070 = Thinking ready: confidential processing

m6a6f75726e616c2e636c692e627261696e2e72656164792e7370702e636865636b6564 = Thinking ready: confidential processing, checked { $age } ago

m6a6f75726e616c2e636c692e627261696e2e72656164792e7370702e6d6f64656c = Thinking ready: confidential processing { $model }

m6a6f75726e616c2e636c692e627261696e2e72656164792e7370702e6d6f64656c2e636865636b6564 = Thinking ready: confidential processing { $model }, checked { $age } ago

m6a6f75726e616c2e636c692e627261696e2e72656164792e756e6b6e6f776e = Thinking ready: unknown

m6a6f75726e616c2e636c692e627261696e2e72656164792e756e6b6e6f776e2e636865636b6564 = Thinking ready: unknown, checked { $age } ago

m6a6f75726e616c2e636c692e627261696e2e72656164792e756e6b6e6f776e2e6d6f64656c = Thinking ready: unknown { $model }

m6a6f75726e616c2e636c692e627261696e2e72656164792e756e6b6e6f776e2e6d6f64656c2e636865636b6564 = Thinking ready: unknown { $model }, checked { $age } ago

m6a6f75726e616c2e636c692e627261696e2e736574746c656d656e742e66696e6765727072696e742e6368616e676564 = protected fingerprint key changed after validation

m6a6f75726e616c2e636c692e627261696e2e736574746c656d656e742e7075626c69636174696f6e2e6368616e676564 = credential validation changed after publication

m6a6f75726e616c2e636c692e627261696e2e736574746c656d656e742e726f6f742e6368616e676564 = admitted journal path changed

m6a6f75726e616c2e636c692e627261696e2e736574746c656d656e742e73656c656374696f6e2e6368616e676564 = credential selection changed during refresh

m6a6f75726e616c2e636c692e627261696e2e736574746c656d656e742e756e636f6e6669726d6564 = credential validation publication was not confirmed

m6a6f75726e616c2e636c692e627261696e2e736574746c656d656e742e76616c69646174696f6e2e6368616e676564 = credential validation changed after refresh

m6a6f75726e616c2e636c692e627261696e2e7573616765 = usage: { "solstone journal brain [-h] {status,refresh} ..." }

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e636865636b696e672e6167677265676174652e72657175697265732e6d61746368696e672e636865636b696e672e6d61726b6572 = checking aggregate requires matching checking marker

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e636865636b696e672e726561736f6e2e69732e6e6f742e76616c69642e636f6d706f6e656e742e65766964656e6365 = checking reason is not valid component evidence

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e636f6d706f6e656e742e7374617475732e646f65732e6e6f742e6d617463682e726561736f6e2e616767726567617465 = component status does not match reason aggregate

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e64657461696c = { $field }: { $reason }

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e646961676e6f737469632e656e756d2e76616c75652e6e6f742e616c6c6f776564 = diagnostic enum value not allowed

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e646961676e6f737469632e6b65792e6e6f742e616c6c6f776564 = diagnostic key not allowed

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e646961676e6f737469632e76616c75652e6d7573742e62652e616e2e656e756d2e737472696e67 = diagnostic value must be an enum string

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e646f65732e6e6f742e6d617463682e6167677265676174652e7374617465 = does not match { "aggregate_state" }

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e65787065637465642e7368613235362e6865782e737472696e67 = expected { "SHA-256" } hex string

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e69732e72657175697265642e7768656e2e7374617475732e69732e6f6b = is required when status is ok

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e6d697373696e672e6c616e652e6170706c696361626c652e65766964656e63652e776974686f75742e6869676865722e7072696f726974792e6361757365 = missing lane-applicable evidence without higher-priority cause

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e6d7573742e62652e612e6e6f6e2e6e656761746976652e696e7465676572 = must be a non-negative integer

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e6d7573742e62652e612e7368613235362e6865782e646967657374 = must be a { "SHA-256" } hex digest

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e6d7573742e62652e612e737472696e67 = must be a string

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e6d7573742e62652e612e737472696e672e6f722e6e756c6c = must be a string or null

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e6d7573742e62652e616e2e696e7465676572 = must be an integer

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e6d7573742e62652e616e2e69736f383630312e74696d657374616d70 = must be an { "ISO-8601" } timestamp

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e6d7573742e62652e616e2e6f626a656374 = must be an object

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e6d7573742e62652e65766964656e63652e7265636f726461626c65 = must be evidence-recordable

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e6d7573742e62652e6e6f6e2e656d707479 = must be non-empty

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e6d7573742e62652e6e756c6c2e7768656e2e6167677265676174652e73746174652e69732e7265616479 = must be null when { "aggregate_state" } is ready

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e6d7573742e62652e70726f76696465722e6b65792e696e76616c6964 = must be { "provider_key_invalid" }

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e6d7573742e62652e7365742e7768656e2e6167677265676174652e73746174652e69732e6e6f742e7265616479 = must be set when { "aggregate_state" } is not ready

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e6e6f6e2e6f6b2e65766964656e63652e72657175697265732e726561736f6e = non-ok evidence requires reason

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e6e6f742e617474656d707465642e69732e6f6e6c792e76616c69642e666f722e67656e6572617465 = { "not_attempted" } is only valid for generate

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e6e6f742e617474656d707465642e726561736f6e2e6d7573742e7265706561742e63617573616c2e7072657265717569736974652e726561736f6e = { "not_attempted" } reason must repeat causal prerequisite reason

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e6e6f742e617474656d707465642e72657175697265732e612e6e6f6e2e6f6b2e707265726571756973697465 = { "not_attempted" } requires a non-ok prerequisite

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e6f6b2e65766964656e63652e72657175697265732e6e756c6c2e726561736f6e = ok evidence requires null reason

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e7072657265717569736974652e72656e6577616c2e72657175697265732e6f6b2e6f722e6465636c617265642e6661696c757265 = prerequisite renewal requires ok or declared failure

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e726561736f6e2e6e6f742e616c6c6f7765642e666f722e65766964656e63652e636f6d706f6e656e74 = reason not allowed for evidence component

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e7265636f72642e6167677265676174652e726561736f6e2e646f65732e6e6f742e6d617463682e65766964656e6365 = record aggregate/reason does not match evidence

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e756e6b6e6f776e2e6669656c64 = unknown field

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e756e6b6e6f776e2e746f702e6c6576656c2e6669656c64 = unknown top-level field

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e756e6b6e6f776e2e76616c7565 = unknown value

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e756e737570706f727465642e62696e64696e672e76657273696f6e = unsupported binding version

m6a6f75726e616c2e636c692e627261696e2e76616c69646174696f6e2e756e737570706f727465642e736368656d612e76657273696f6e = unsupported schema version

m6a6f75726e616c2e636c692e627261696e2e7772697465722e6163746976652e627261696e2e66696e6765727072696e742e69732e756e617661696c61626c65 = active brain fingerprint is unavailable

m6a6f75726e616c2e636c692e627261696e2e7772697465722e627261696e2e6162616e646f6e2e726561736f6e2e69732e6e6f742e7265636f726461626c652e65766964656e6365 = brain abandon reason is not recordable evidence

m6a6f75726e616c2e636c692e627261696e2e7772697465722e627261696e2e66696e6765727072696e742e6368616e676564 = brain fingerprint changed

m6a6f75726e616c2e636c692e627261696e2e7772697465722e627261696e2e66696e6765727072696e742e6b65792e69732e756e617661696c61626c65 = brain fingerprint key is unavailable

m6a6f75726e616c2e636c692e627261696e2e7772697465722e627261696e2e6c616e652e6368616e676564 = brain lane changed

m6a6f75726e616c2e636c692e627261696e2e7772697465722e627261696e2e7072657265717569736974652e65766964656e63652e69732e756e73616665 = brain prerequisite evidence is unsafe

m6a6f75726e616c2e636c692e627261696e2e7772697465722e627261696e2e7265636f72642e6368616e6765642e647572696e672e696e7370656374696f6e = Brain record changed during inspection

m6a6f75726e616c2e636c692e627261696e2e7772697465722e627261696e2e726566726573682e636865636b696e672e6d61726b65722e69732e616273656e74 = brain refresh checking marker is absent

m6a6f75726e616c2e636c692e627261696e2e7772697465722e627261696e2e726566726573682e7065726d69742e65787069726564 = brain refresh permit expired

m6a6f75726e616c2e636c692e627261696e2e7772697465722e627261696e2e726566726573682e7265636f72642e7265766973696f6e2e6368616e676564 = brain refresh record revision changed

m6a6f75726e616c2e636c692e627261696e2e7772697465722e627261696e2e726566726573682e7265766973696f6e2e6368616e676564 = brain refresh revision changed

m6a6f75726e616c2e636c692e627261696e2e7772697465722e627261696e2e726566726573682e72756e2e69642e6368616e676564 = brain refresh run id changed

m6a6f75726e616c2e636c692e627261696e2e7772697465722e627261696e2e72756e74696d652e6661696c7572652e6d61726b65722e6368616e676564 = brain runtime failure marker changed

m6a6f75726e616c2e636c692e627261696e2e7772697465722e636f6e66696775726174696f6e2e72656a656374696f6e2e6368616e6765642e61667465722e7075626c69636174696f6e = configuration rejection changed after publication

m6a6f75726e616c2e636c692e627261696e2e7772697465722e63726564656e7469616c2e617574686f726974792e6368616e6765642e647572696e672e696e7370656374696f6e = credential authority changed during inspection

m6a6f75726e616c2e636c692e627261696e2e7772697465722e63726564656e7469616c2e617574686f726974792e6368616e6765642e647572696e672e6f62736572766174696f6e = credential authority changed during observation

m6a6f75726e616c2e636c692e627261696e2e7772697465722e63726564656e7469616c2e617574686f726974792e69732e6e6f742e616e2e6f626a656374 = credential authority is not an object

m6a6f75726e616c2e636c692e627261696e2e7772697465722e63726564656e7469616c2e617574686f726974792e69732e756e7265616461626c65 = credential authority is unreadable

m6a6f75726e616c2e636c692e627261696e2e7772697465722e63726564656e7469616c2e62696e64696e672e617574686f726974792e636f756c642e6e6f742e62652e726561642e6572726f72 = credential binding authority could not be read: { $error }

m6a6f75726e616c2e636c692e627261696e2e7772697465722e63726564656e7469616c2e686f6c642e617574686f726974792e6368616e676564 = credential hold authority changed

m6a6f75726e616c2e636c692e627261696e2e7772697465722e63726564656e7469616c2e686f6c642e617574686f726974792e69732e6e6f742e616e2e6f626a656374 = credential hold authority is not an object

m6a6f75726e616c2e636c692e627261696e2e7772697465722e63726564656e7469616c2e686f6c642e617574686f726974792e69732e756e7265616461626c652e6572726f72 = credential hold authority is unreadable: { $error }

m6a6f75726e616c2e636c692e627261696e2e7772697465722e63726564656e7469616c2e6f62736572766174696f6e2e617574686f726974792e69732e6e6f742e616e2e6f626a656374 = credential observation authority is not an object

m6a6f75726e616c2e636c692e627261696e2e7772697465722e63726564656e7469616c2e6f62736572766174696f6e2e617574686f726974792e69732e756e7265616461626c65 = credential observation authority is unreadable

m6a6f75726e616c2e636c692e627261696e2e7772697465722e63726564656e7469616c2e6f62736572766174696f6e2e6368616e6765642e647572696e672e72656672657368 = credential observation changed during refresh

m6a6f75726e616c2e636c692e627261696e2e7772697465722e63726564656e7469616c2e6f62736572766174696f6e2e6368616e6765642e647572696e672e776f726b = credential observation changed during work

m6a6f75726e616c2e636c692e627261696e2e7772697465722e63726564656e7469616c2e6f62736572766174696f6e2e6964656e746974792e69732e696e76616c6964 = credential observation identity is invalid

m6a6f75726e616c2e636c692e627261696e2e7772697465722e63726564656e7469616c2e6f62736572766174696f6e2e6c61636b732e6f726967696e616c2e62696e64696e67 = credential observation lacks original binding

m6a6f75726e616c2e636c692e627261696e2e7772697465722e63726564656e7469616c2e6f62736572766174696f6e732e6172652e6e6f742e616e2e6f626a656374 = credential observations are not an object

m6a6f75726e616c2e636c692e627261696e2e7772697465722e63726564656e7469616c2e70726f636573732e73746174652e6368616e6765642e647572696e672e696e7370656374696f6e = credential process state changed during inspection

m6a6f75726e616c2e636c692e627261696e2e7772697465722e63726564656e7469616c2e7075626c69636174696f6e2e7265736572766174696f6e2e69732e616273656e74 = credential publication reservation is absent

m6a6f75726e616c2e636c692e627261696e2e7772697465722e63726564656e7469616c2e72656a656374696f6e2e6368616e6765642e61667465722e7075626c69636174696f6e = credential rejection changed after publication

m6a6f75726e616c2e636c692e627261696e2e7772697465722e63726564656e7469616c2e72656a656374696f6e2e6368616e6765642e647572696e672e72656672657368 = credential rejection changed during refresh

m6a6f75726e616c2e636c692e627261696e2e7772697465722e63726564656e7469616c2e72656a656374696f6e2e6368616e6765642e647572696e672e76616c69646174696f6e = credential rejection changed during validation

m6a6f75726e616c2e636c692e627261696e2e7772697465722e6669656c642e6d7573742e62652e616e2e6f626a656374 = { $field } must be an object

m6a6f75726e616c2e636c692e627261696e2e7772697465722e70726f7465637465642e66696e6765727072696e742e6b65792e6368616e6765642e61667465722e72656a656374696f6e = protected fingerprint key changed after rejection

m6a6f75726e616c2e636c692e627261696e2e7772697465722e70726f7465637465642e66696e6765727072696e742e6b65792e6368616e6765642e647572696e672e61646d697373696f6e = protected fingerprint key changed during admission

m6a6f75726e616c2e636c692e627261696e2e7772697465722e70726f7465637465642e66696e6765727072696e742e6b65792e6368616e6765642e647572696e672e696e7370656374696f6e = protected fingerprint key changed during inspection

m6a6f75726e616c2e636c692e627261696e2e7772697465722e70726f7465637465642e66696e6765727072696e742e6b65792e6368616e6765642e647572696e672e76616c69646174696f6e = protected fingerprint key changed during validation

m6a6f75726e616c2e636c692e627261696e2e7772697465722e70726f7465637465642e66696e6765727072696e742e6b65792e69732e616273656e742e7768696c652e612e63726564656e7469616c2e62696e64696e672e657869737473 = protected fingerprint key is absent while a credential binding exists

m6a6f75726e616c2e636c692e627261696e2e7772697465722e70726f7465637465642e66696e6765727072696e742e6b65792e69732e616273656e742e7768696c652e63726564656e7469616c2e617574686f726974792e657869737473 = protected fingerprint key is absent while credential authority exists

m6a6f75726e616c2e636c692e627261696e2e7772697465722e70726f7465637465642e66696e6765727072696e742e6b65792e69732e756e617661696c61626c65 = protected fingerprint key is unavailable

m6a6f75726e616c2e636c692e627261696e2e7772697465722e726561736f6e2e69732e6e6f742e7265636f726461626c652e65766964656e6365 = reason is not recordable evidence

m6a6f75726e616c2e636c692e627261696e2e7772697465722e72656a65637465642e726566726573682e726563656970742e646f65732e6e6f742e6d617463682e6974732e70726f6265 = rejected refresh receipt does not match its probe

m6a6f75726e616c2e636c692e627261696e2e7772697465722e72656a656374696f6e2e726563656970742e726f6f742e70726f76696465722e6368616e676564 = rejection receipt root/provider changed

m6a6f75726e616c2e636c692e627261696e2e7772697465722e736574746c65642e63726564656e7469616c2e6f62736572766174696f6e2e6368616e6765642e647572696e672e776f726b = settled credential observation changed during work

m6a6f75726e616c2e636c692e627261696e2e7772697465722e76616c69646174696f6e2e726573756c742e646f65732e6e6f742e6d617463682e6974732e63726564656e7469616c = validation result does not match its credential

m6a6f75726e616c2e636c692e627261696e2e7772697465722e76616c69646174696f6e2e726573756c742e69732e6e6f742e616e2e6f626a656374 = validation result is not an object

m6a6f75726e616c2e636c692e636c69656e742e6572726f722e6d616c666f726d6564 = the journal response couldn't be read.

m6a6f75726e616c2e636c692e636c69656e742e6572726f722e74696d656f7574 = The journal didn't answer in time.

m6a6f75726e616c2e636c692e636c69656e742e6572726f722e756e726561636861626c65 = your journal couldn't be reached over { "HTTP" }.

m6a6f75726e616c2e636c692e636c69656e742e6572726f722e756e7265616461626c65 = The journal returned an unreadable error.

m6a6f75726e616c2e636c692e636c69656e742e706f72742e696e76616c6964 = { "SOLSTONE_CONVEY_PORT" } must be an integer from 1 to 65535.

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e = journal process admission failed: { $detail }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e62696e64696e672e6d69736d61746368 = launch installation binding mismatch

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e62697274682e696e76616c6964 = { "linux" } process birth has a malformed { "boot_id" }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e62697274682e706f736974696f6e = { "linux" } process birth has a malformed { "boot_id" } at line { $line } column { $column }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e627974652e636f756e74 = launch pipe returned an invalid byte count

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e636c65616e75702e70656e64696e67 = previous launch { "I/O" } cleanup remains incomplete

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e636f6d6d69742e6d69736d61746368 = launch admission commit mismatch

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e646561646c696e652e656c6170736564 = launch deadline elapsed

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e646561646c696e652e696e76616c6964 = expired or invalid { "Windows" } launch deadline

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e64657363726970746f722e656e636f64696e67 = non-{ "Unicode" } { "Windows" } launch descriptor

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e64657363726970746f722e73697a65 = oversized { "Windows" } launch descriptor

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e656e64706f696e74 = invalid launch endpoint

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e656e7472792e686f73746564 = installed task at hosted entry

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e656e7472792e6d69736d61746368 = launch variant or installed action mismatch

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6672616d652e73697a65 = invalid launch frame size

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6772616e742e616363657373 = launch handle access or type does not match its grant

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6772616e742e6469736b = launch grant is not a disk file

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6772616e742e6475706c6963617465 = duplicate launch handle or grant kind

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6772616e742e6f7264696e617279 = launch grant is not an ordinary disk file

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e67756172642e6475706c6963617465 = installation guard: duplicate identity guard { $field }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e67756172642e656e636f64696e67 = non-{ "Unicode" } installation guard

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e67756172642e696e76616c6964 = installation guard: invalid identity guard { $field }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e67756172642e6d69736d61746368 = launch guard tuple differs from inherited guard

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e67756172642e6d697373696e67 = installation guard: identity guard fields are incomplete

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e68616e646c652e696e76616c6964 = invalid transferred handle

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e696f2e6c6f63616c2e666f6c646572 = resolve { "LocalAppData" } known folder: { $error }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e696f2e6d61726b65722e636c6f6e65 = clone adoption marker: { $error }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e696f2e6d61726b65722e72656164 = read adoption marker: { $error }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e696f2e6d61726b65722e73746174 = stat adoption marker: { $error }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e696f2e6e616d6573706163652e72656164 = read namespace directory: { $error }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e696f2e6f776e65722e6f70656e = open owner directory: { $error }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e696f2e7265636f72642e72656164 = read identity record: { $error }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e696f2e7265636f72642e73746174 = stat identity record: { $error }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e696f2e726f6f74 = canonicalize root: { $error }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e696f2e73746f726167652e6174747273 = query storage attributes: { $error }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e696f2e73746f726167652e66696c65 = open storage file: { $error }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e696f2e73746f726167652e6964656e74697479 = query storage identity: { $error }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e696f2e73746f726167652e6c6f636b = lock identity storage: { $error }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e696f2e73746f726167652e6f70656e = open storage directory: { $error }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e6c6f63616c2e656e636f64696e67 = { "LocalAppData" } known folder is not valid { "UTF-16" }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e6d61726b65722e636f6e74656e74 = adoption marker has invalid content

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e6d61726b65722e6d697373696e67 = committed record has no adoption marker

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e6d61726b65722e6f727068616e = orphan adoption marker

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e6d61726b65722e73697a65 = adoption marker has invalid size

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e6e616d6573706163652e6368696c64 = unknown namespace child

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e6f776e65722e706f69736f6e6564 = owner coordinator is poisoned

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e706174682e646576696365 = { "Windows" } verbatim and device paths are not accepted

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e706174682e656e636f64696e67 = { "Windows" } path is not valid { "UTF-16" }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e706174682e657363617065 = { "Windows" } path escapes its absolute root

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e706174682e6e756c = { "Windows" } path contains an interior { "NUL" }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e706174682e72656c6174697665 = { "Windows" } path must be fully qualified

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e706174682e736572766572 = { "Windows" } { "UNC" } path has no server

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e706174682e7368617265 = { "Windows" } { "UNC" } path has no share

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e706174682e73697a65 = path exceeds { "4096" } bytes

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e7072657061726564 = installation identity record is prepared

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e7265636f72642e6d697373696e67 = namespace record is missing

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e7265636f72642e73697a65 = identity record exceeds bounds

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e726f6f742e6d69736d61746368 = record does not bind the requested root

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e73746f726167652e6469726563746f7279 = storage path is not a directory

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e73746f726167652e66696c65 = storage path is not a regular file

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e73746f726167652e72657061727365 = storage path is a reparse point

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6964656e746974792e746f6d6273746f6e6564 = installation identity record is tombstoned

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6e6f6e6365 = invalid launch endpoint nonce

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6f62736f6c657465 = obsolete { "Windows" } launch marker

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e6f666665722e6d69736d61746368 = launch offer provenance mismatch

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e706172656e742e6d69736d61746368 = launch server is not the exact direct parent

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e706172656e742e70726f76656e616e6365 = invalid parent launch provenance

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e706565722e756e7665726966696564 = launch pipe peer is not verifiably live

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e70726f636573732e6372656174696f6e = current { "Windows" } process creation { "FILETIME" } is unavailable

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e70726f636573732e7369676e616c6564 = current { "Windows" } process is signaled

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e70726f76656e616e6365 = invalid launch provenance

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e7265636f72642e6173636969 = identity record is not { "ASCII" }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e7265636f72642e636865636b73756d = identity record checksum mismatch

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e7265636f72642e6669656c64 = invalid identity record { $field }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e7265636f72642e6c696e6573 = identity record is not exactly eight { "LF" } lines

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e7265636f72642e73697a65 = identity record exceeds { "24 KiB" }

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e726f6f742e6d697373696e67 = launch executable has no installation root

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e7365727665722e6d69736d61746368 = launch pipe server identity mismatch

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e736e617073686f742e616273656e74 = current process is absent from the parent snapshot

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e7461736b2e70726f76656e616e6365 = invalid installed task provenance

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e776169742e636f6d706c657465 = launch { "I/O" } wait failed: { $error }; { "I/O" } cancellation completed

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e776169742e646561646c696e652e636f6d706c657465 = launch deadline elapsed; { "I/O" } cancellation completed

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e776169742e646561646c696e652e696e636f6d706c657465 = launch deadline elapsed; { "I/O" } cleanup incomplete and retained

m6a6f75726e616c2e636c692e6465706963742e61646d697373696f6e2e776169742e696e636f6d706c657465 = launch { "I/O" } wait failed: { $error }; { "I/O" } cleanup incomplete and retained

m6a6f75726e616c2e636c692e6465706963742e646574656374696f6e2e756e617661696c61626c65 = Object detection is unavailable. Run `{ "solstone journal install-models" }` to check or repair the { "RF-DETR" } assets.

m6a6f75726e616c2e636c692e6465706963742e646574656374696f6e2e756e617661696c61626c652e77696e646f7773 = object detection is unavailable. repair or reinstall your journal.

m6a6f75726e616c2e636c692e6465706963742e68656c70 = usage: { "solstone journal depict [-h] [--redo] [-v] [-d] FILE" }

m6a6f75726e616c2e636c692e6465706963742e6d657461646174612e696e76616c6964 = native depict: invalid { "SEGMENT_META" } { "JSON" }

m6a6f75726e616c2e636c692e64657363726962652e63616e63656c6c6564 = describe cancelled

m6a6f75726e616c2e636c692e64657363726962652e6465636f64652e6661696c6564 = failed to decode video: { $path }

m6a6f75726e616c2e636c692e64657363726962652e6465666572726564 = describe deferred

m6a6f75726e616c2e636c692e64657363726962652e64656665727265642e726561736f6e = describe deferred: { $reason }

m6a6f75726e616c2e636c692e64657363726962652e66616365732e736b6970706564 = face enrichment skipped: { $kind }

m6a6f75726e616c2e636c692e64657363726962652e66696c652e656e636f64696e67 = video path must have a { "UTF-8" } filename

m6a6f75726e616c2e636c692e64657363726962652e6f75747075742e656e636f6465 = failed to encode output: { $error }

m6a6f75726e616c2e636c692e64657363726962652e6f75747075742e7772697465 = failed to write output: { $error }

m6a6f75726e616c2e636c692e64657363726962652e706174682e636f6e666967 = could not read journal path: { $reason }

m6a6f75726e616c2e636c692e64657363726962652e706174682e686f6d65 = could not resolve home directory: { $reason }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e666c61742e626f756e64 = bound directory name exists and is not a directory

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e666c61742e6372656174652e666c61742e6469726563746f72792e6368696c64 = create flat-directory child failed for { $path }: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e666c61742e6e6f726d616c = name must be one normal component

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e666c61742e726561642e666c61742e6469726563746f72792e6964656e74697479 = read flat-directory identity failed for { $path }: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e666c61742e72656c6174697665 = path must be a nonempty sequence of normal portable components

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e666c61742e726576616c69646174652e666c61742e6469726563746f7279 = revalidate flat directory failed for { $path }: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e666c61742e737461742e666c61742e6469726563746f72792e64657363656e64616e74 = stat flat-directory descendant failed for { $path }: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e666c61742e737461742e6f70656e65642e666c61742e6469726563746f72792e64657363656e64616e74 = stat opened flat-directory descendant failed for { $path }: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e666c61742e75746638 = the name is not valid { "UTF-8" } text

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e6672616d65732e6661696c6564 = all describe frame requests failed

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e696e7374616c6c2e6f6273657276652e696e7374616c6c65642e64657374696e6174696f6e2e61667465722e6d6f7665 = { $path }: observe installed destination after move: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e696e7374616c6c2e7265636f6e63696c652e696e7374616c6c2e61667465722e6661696c65642e6d6f7665 = { $path }: reconcile install after failed move: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e696e7374616c6c2e726576616c69646174652e696e7374616c6c2e70617468732e61667465722e6d6f7665 = { $path }: revalidate install paths after move: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e6a6f6273 = { "--jobs" } must be positive

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e6d657267652e6672616d65 = missing merge frame

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e6d657461646174612e6f626a656374 = { "SEGMENT_META" } must be an object

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e6f63722e636f766572616765 = screen { "OCR" } coverage is incomplete; private frame diagnostics record the gap

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e6f63722e7075626c69636174696f6e = screen { "OCR" } publication failed: { $kind }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e6f63722e726574697265 = screen { "OCR" } stale aggregate could not be retired: { $kind }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e706172656e74 = video has no parent

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e70726f6772657373 = describe frame window made no progress

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e616e63686f722e69732e6e6f742e612e6d6f756e742e6d616e616765722e766f6c756d652e677569642e70617468 = anchor is not a { "Mount-Manager" } volume { "GUID" } path

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e6368616e676564 = prepared publication changed

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e636f6d706f6e656e74 = publication leaf is not a plain component

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e63757272656e742e6469726563746f72792e69732e6e6f742e76616c69642e757466 = current directory is not valid { "UTF-8" }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e64657374696e6174696f6e = publication destination is not a plain file

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e64657374696e6174696f6e2e616c72656164792e6964656e7469666965732e7468652e736f757263652e66696c65 = destination already identifies the source file

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e64657374696e6174696f6e2e6368616e6765642e647572696e672e7075626c69636174696f6e = destination changed during publication

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e64657374696e6174696f6e2e6861732e6e6f2e6e6f726d616c2e6e616d65 = destination has no normal name

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e64657374696e6174696f6e2e69732e612e726570617273652e706f696e74 = destination is a reparse point

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e64657374696e6174696f6e2e69732e6e6f742e612e726567756c61722e66696c65 = destination is not a regular file

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e6964656e74697479 = prepared publication identity changed

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e6964656e746974792e76616c75652e6f75742e6f662e72616e6765 = identity value out of range

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e696e7374616c6c2e7075626c69636174696f6e2e657868617573746564 = install publication exhausted

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e6c656166 = publication leaf missing

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e6d6f64652e657863656564732e6f = mode exceeds { "0o777" }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e6d6f76652e7370656c6c696e672e636f6e7461696e732e616e2e696e746572696f722e6e756c = move spelling contains an interior { "NUL" }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e706172656e74 = publication parent missing

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e70617468 = { $path }: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e7075626c69636174696f6e2e706174682e69732e6e6f742e76616c69642e757466 = publication path is not valid { "UTF-8" }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e7075626c69736865642e64657374696e6174696f6e2e6964656e746974792e646f65732e6e6f742e6d617463682e7468652e6c6976652e736f75726365 = published destination identity does not match the live source

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e7075626c69736865642e64657374696e6174696f6e2e69732e6e6f742e612e726567756c61722e66696c65 = published destination is not a regular file

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e7075626c69736865642e64657374696e6174696f6e2e6e616d652e69732e616273656e74 = published destination name is absent

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e736f757263652e616e642e64657374696e6174696f6e2e6172652e6f6e2e646966666572656e742e766f6c756d6573 = source and destination are on different volumes

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e736f757263652e616e642e64657374696e6174696f6e2e6e616d652e7468652e73616d652e66696c65 = source and destination name the same file

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e736f757263652e6861732e6e6f2e6e6f726d616c2e6e616d65 = source has no normal name

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e736f757263652e6964656e746974792e6368616e6765642e6265666f72652e7075626c69636174696f6e = source identity changed before publication

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e736f757263652e6964656e746974792e69732e6e65676174697665 = source identity is negative

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e736f757263652e6e616d652e6368616e6765642e6265666f72652e7075626c69636174696f6e = source name changed before publication

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e736f757263652e6e616d652e69732e616273656e742e6265666f72652e7075626c69636174696f6e = source name is absent before publication

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e73796e63 = describe publication parent sync failed: { $error }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7075626c6973682e756e6365727461696e = { $path }: { $operation }: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7265617070656172 = describe frame { $frame } did not reappear with the same timestamp

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7265636f72642e706172656e742e6368616e676564 = screen text parent changed

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7265636f72642e706172656e742e706c61696e = screen text parent is not a plain directory

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72656f70656e = describe could not reopen the video to materialize frame payloads

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726573697a65 = failed to resize categorization frame

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726573706f6e73652e6964 = session response omitted id

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726573706f6e73652e756e6d617463686564 = uncorrelated response

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e62696e64696e67 = { "OCR" } retry frame binding changed

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e65766964656e63652e6d697373696e67 = missing retry evidence

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e68616e646c6572 = native failure handler changed

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e6964 = native retry identity is not { "UTF-8" }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e696e636f6d706c657465 = { "OCR" } retry did not reach the last qualified frame

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e696e7075742e6368616e676564 = retry input changed while reading

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e696e7075742e726567756c6172 = retry input is not a regular file

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e696f2e616c6c6f63617465 = could not allocate stage

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e696f2e646573742e6e616d657370616365 = destination namespace changed before publication

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e696f2e66696c652e6b696e64 = entry is not a regular file

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e696f2e66696c652e72657061727365 = entry is a reparse point

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e696f2e6673796e63 = parent-directory fsync degraded for { $path }: { $error }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e696f2e706172656e742e6964656e74697479 = bound parent identity changed before publication

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e696f2e706172656e742e6b696e64 = parent is not a real directory

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e696f2e706172656e742e72656e616d65 = parent pathname changed before publication

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e696f2e706172656e742e72657061727365 = parent is a reparse point

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e696f2e70726570617265 = stage identity changed while preparing

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e696f2e73746167652e6964656e74697479 = stage identity changed before publication

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e696f2e73746167652e6e616d657370616365 = stage namespace changed before publication

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e696f2e746d70 = could not allocate unique temporary file

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e6a6f75726e616c = native retry journal does not match

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e6f70656e = could not reopen video for { "OCR" } retry

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e7065726d697373696f6e2e6368616e676564 = native retry permission changed

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e72657472792e7065726d697373696f6e2e6d697373696e67 = native retry permission is missing

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e63616e6f6e6963616c697a652e6a6f75726e616c2e726f6f74 = canonicalize journal root: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e636c6f75642e66696c65732e73796e632e726f6f742e7374617475732e636f756c642e6e6f742e62652e7665726966696564 = { "Cloud Files" } sync-root status could not be verified

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e66696c6573797374656d2e69732e6e6f742e6e7466732e6f722e72656673 = filesystem is not { "NTFS" } or { "ReFS" }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e66696c6573797374656d2e747970652e636f756c642e6e6f742e62652e7665726966696564 = filesystem type could not be verified

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e6a6f75726e616c2e726f6f742e636f6e7461696e732e612e726570617273652e706f696e74 = journal root contains a reparse point

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e6a6f75726e616c2e726f6f742e636f6e7461696e732e616e2e696e746572696f722e6e756c = journal root contains an interior { "NUL" }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e6a6f75726e616c2e726f6f742e6861732e616e2e696e76616c69642e706f727461626c652e6e616d65 = journal root has an invalid portable name

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e6a6f75726e616c2e726f6f742e69732e612e636c6f75642e66696c65732e73796e632e726f6f74 = journal root is a { "Cloud Files" } sync root

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e6a6f75726e616c2e726f6f742e69732e612e726570617273652e706f696e74 = journal root is a reparse point

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e6a6f75726e616c2e726f6f742e69732e6f6e2e612e6e6f6e2e6c6f63616c2e766f6c756d65 = journal root is on a non-local volume

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e6a6f75726e616c2e726f6f742e69732e6f6e2e612e72616d2e6469736b = journal root is on a { "RAM" } disk

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e6a6f75726e616c2e726f6f742e69732e6f6e2e72656d6f7661626c652e6f722e6f70746963616c2e6d65646961 = journal root is on removable or optical media

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e6a6f75726e616c2e726f6f742e6d7573742e62652e6c65786963616c6c792e63616e6f6e6963616c = journal root must be lexically canonical

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e6a6f75726e616c2e726f6f742e6d7573742e7573652e612e66756c6c792e7175616c69666965642e64726976652e70617468 = journal root must use a fully qualified drive path

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e6a6f75726e616c2e726f6f742e766f6c756d652e747970652e69732e756e6b6e6f776e = journal root volume type is unknown

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e6f70656e2e66696c6573797374656d2e726f6f74 = open filesystem root: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e6f70656e2e6a6f75726e616c2e6469726563746f7279 = open journal directory: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e6f70656e2e6a6f75726e616c2e726f6f74 = open journal root: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e6f70656e2e6a6f75726e616c2e726f6f742e616e636573746f72 = open journal root ancestor: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e6f70656e2e6a6f75726e616c2e726f6f742e6e616d6573706163652e7761746368 = open journal root namespace watch: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e6f70656e2e76657269666965642e64726976652e726f6f74 = open verified drive root: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e6f70656e2e76657269666965642e6a6f75726e616c2e726f6f74 = open verified journal root: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e71756572792e6a6f75726e616c2e726f6f742e616e636573746f722e61747472696275746573 = query journal root ancestor attributes: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e71756572792e6a6f75726e616c2e726f6f742e61747472696275746573 = query journal root attributes: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e71756572792e6a6f75726e616c2e726f6f742e64726976652e74797065 = query journal root drive type: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e71756572792e6a6f75726e616c2e726f6f742e6964656e74697479 = query journal root identity: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e71756572792e6a6f75726e616c2e726f6f742e6e616d6573706163652e77617463682e61747472696275746573 = query journal root namespace-watch attributes: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e71756572792e6a6f75726e616c2e726f6f742e6e616d6573706163652e77617463682e6964656e74697479 = query journal root namespace-watch identity: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e71756572792e72657461696e65642e6a6f75726e616c2e726f6f742e61747472696275746573 = query retained journal root attributes: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e71756572792e72657461696e65642e6a6f75726e616c2e726f6f742e6964656e74697479 = query retained journal root identity: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e71756572792e76657269666965642e64726976652e726f6f742e61747472696275746573 = query verified drive root attributes: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e71756572792e76657269666965642e64726976652e726f6f742e6964656e74697479 = query verified drive root identity: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e71756572792e76657269666965642e6a6f75726e616c2e726f6f742e61747472696275746573 = query verified journal root attributes: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e71756572792e76657269666965642e6a6f75726e616c2e726f6f742e6964656e74697479 = query verified journal root identity: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e726561642e736f757263652e66696c652e6964656e74697479 = read source file identity: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e7265737461742e61637175697265642e6a6f75726e616c2e726f6f74 = restat acquired journal root: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e737461742e61637175697265642e6a6f75726e616c2e726f6f74 = stat acquired journal root: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e737461742e63616e6f6e6963616c2e726f6f742e636f6d706f6e656e74 = stat canonical root component: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e737461742e6f70656e65642e63616e6f6e6963616c2e726f6f742e636f6d706f6e656e74 = stat opened canonical root component: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f6f742e737461742e6f70656e65642e66696c6573797374656d2e726f6f74 = stat opened filesystem root: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e726f772e6672616d65 = describe row omitted { "frame_id" }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e736f757263652e6279746573 = { "OCR" } source bytes changed

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e736f757263652e6368616e676564 = { "OCR" } source changed before publication

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e736f757263652e6964656e74697479 = { "OCR" } source identity changed

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e736f757263652e6f70656e = { "OCR" } source changed while opening

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e736f757263652e706c61696e = { "OCR" } source is not a plain file

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e74656d70 = could not allocate describe temp file

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e74696d657374616d70 = describe frame { $frame } omitted timestamp

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e636c65616e7570 = { $detail }; could not remove stage { $stage }: { $error }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e6e616d652e616c7465726e6174652e646174612e73747265616d = the name contains a colon

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e6e616d652e636f6e74726f6c = the name contains a control character

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e6e616d652e646f742e636f6d706f6e656e74 = '.' and '..' are not valid names

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e6e616d652e656d707479 = the name is empty

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e6e616d652e666f7262696464656e2e636861726163746572 = the name contains a character { "Windows" } does not allow

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e6e616d652e72657365727665642e646576696365 = the name is reserved by { "Windows" }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e6e616d652e726f6f742e6f722e707265666978 = the name must be a single relative component

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e6e616d652e736570617261746f72 = the name contains a path separator

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e6e616d652e746f6f2e6c6f6e67 = the name is longer than { "255" } { "UTF-8" } bytes

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e6e616d652e747261696c696e672e646f742e6f722e7370616365 = the name ends in a dot or space

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e616c72656164792e657869737473 = publication path ancestor '{ $component }' appeared during create-only

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e636c6173736966792e696e7374616c6c2e736f75726365 = classify install source: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e636f6d706f6e656e74 = publication path component is not portable: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e6372656174652e7075626c69636174696f6e2e706174682e616e636573746f72 = create publication-path ancestor: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e646f742e636f6d706f6e656e74 = publication path contains '.' or '..'

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e64726976652e72656c6174697665 = publication path is drive-relative

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e656d707479 = publication path is empty

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e6964656e746974792e6368616e676564 = publication path identity changed at retained level { $level }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e6964656e746974792e6d69736d61746368 = publication path ancestor '{ $component }' identity changed between create and reopen

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e696e746572696f722e6e756c = publication path contains an interior { "NUL" }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e6d697373696e67 = publication path component '{ $component }' does not exist

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e6d6f76652e7370656c6c696e672e6469766572676564 = publication path move spelling no longer names the terminal parent

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e6e6f742e6469726563746f7279 = publication path ancestor '{ $component }' is not a directory

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e6e6f742e726567756c61722e66696c65 = publication path leaf '{ $component }' is not a regular file

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e6f70656e2e696e7374616c6c2e736f75726365 = open install source: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e6f70656e2e7075626c69636174696f6e2e706174682e616e636573746f72 = open publication-path ancestor: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e6f70656e2e7075626c69636174696f6e2e706174682e616e63686f72 = open publication-path anchor: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e6f70656e2e7075626c69636174696f6e2e706174682e6d6f76652e7370656c6c696e67 = open publication-path move spelling: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e706174682e746f6f2e6c6f6e67 = publication path exceeds the extended-length bound

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e71756572792e696e7374616c6c2e706174682e61747472696275746573 = query install path attributes: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e71756572792e696e7374616c6c2e736f757263652e6964656e74697479 = query install source identity: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e71756572792e7075626c69636174696f6e2e706174682e61747472696275746573 = query publication-path attributes: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e71756572792e7075626c69636174696f6e2e706174682e6964656e74697479 = query publication-path identity: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e72656f70656e2e7075626c69636174696f6e2e706174682e616e636573746f72 = reopen publication-path ancestor: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e726570617273652e706f696e74 = publication path ancestor '{ $component }' is a reparse point

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e7265736f6c76652e7075626c69636174696f6e2e706174682e63757272656e742e6469726563746f7279 = resolve publication-path current directory: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e726576616c69646174652e696e7374616c6c2e736f757263652e61667465722e636c617373696669636174696f6e = revalidate install source after classification: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e726f6f742e6f6e6c79 = publication path is a drive root with no destination

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e726f6f7465642e776974686f75742e6472697665 = publication path is rooted without a drive

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e756e632e6f722e6465766963652e707265666978 = publication path has a { "UNC" } or device prefix

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e77696e646f77732e706174682e756e737461626c65 = publication path has no { "Mount-Manager" } volume { "GUID" }: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e706970656c696e652e7769746864726177 = screen { "OCR" } withdrawal could not be prepared: { $detail }

m6a6f75726e616c2e636c692e64657363726962652e73657474696e67732e66696e697465 = { $setting } must be a finite number

m6a6f75726e616c2e636c692e64657363726962652e73657474696e67732e696d706f7274616e6365 = { $setting } must be { "ignore" }, { "low" }, { "normal" }, or { "high" }

m6a6f75726e616c2e636c692e64657363726962652e73657474696e67732e6f626a656374 = { $setting } must be an object

m6a6f75726e616c2e636c692e64657363726962652e73657474696e67732e737472696e67 = { $setting } must be a string

m6a6f75726e616c2e636c692e64657363726962652e73657474696e67732e737472696e6773 = { $setting } must be an array of strings

m6a6f75726e616c2e636c692e64657363726962652e73657474696e67732e75696e74 = { $setting } must be an unsigned 32-bit integer

m6a6f75726e616c2e636c692e64657363726962652e73746f7265642e6572726f722e656d707479 = Empty markdown response for { $category }

m6a6f75726e616c2e636c692e64657363726962652e73746f7265642e6572726f722e696e636f6d706c657465 = Extraction never completed

m6a6f75726e616c2e636c692e64657363726962652e73746f7265642e6572726f722e696e76616c6964 = Invalid { "JSON" } response

m6a6f75726e616c2e636c692e64657363726962652e73746f7265642e6572726f722e696e76616c69642e63617465676f7279 = Invalid { "JSON" } response for { $category }: { $error }

m6a6f75726e616c2e636c692e64657363726962652e73746f7265642e6572726f722e726573697a65 = failed to resize extraction frame

m6a6f75726e616c2e636c692e64657363726962652e73746f7265642e6572726f722e736368656d61 = Schema validation failed for { $category }

m6a6f75726e616c2e636c692e64657363726962652e73746f7265642e6572726f722e736368656d612e64657461696c = Schema validation failed for { $category }: { $errors }

m6a6f75726e616c2e636c692e64657363726962652e73746f7265642e6572726f722e7472756e6361746564 = Truncated markdown response for { $category }

m6a6f75726e616c2e636c692e64657363726962652e73746f7265642e6e6f74652e626f756e646564 = [{ "solstone" }: extraction output bounded before journaling - degenerate length sanitized/truncated]

m6a6f75726e616c2e636c692e64657363726962652e73746f7265642e6e6f74652e6f6372 = text recognized from your screens. it may contain recognition errors. this says what was visible, not what you read, typed or did. masking passwords and other secrets is best effort; your redaction rules do not apply.

m6a6f75726e616c2e636c692e64657363726962652e73746f7265642e6f63722e68656164696e67 = { $clock } · { $category }

m6a6f75726e616c2e636c692e64657363726962652e75736167652e617267756d656e742e756e6b6e6f776e = usage: { "solstone journal describe [-h] [--frames-only] [--redo] [-j N] [--journal PATH] [-v] [-d] FILE" }{ "\u000a" }{ "solstone journal describe" }: error: unknown argument: { $argument }

m6a6f75726e616c2e636c692e64657363726962652e75736167652e6261636b66696c6c2e617267756d656e7473 = usage: { "solstone journal describe [-h] [--frames-only] [--redo] [-j N] [--journal PATH] [-v] [-d] FILE" }{ "\u000a" }{ "solstone journal describe" }: error: face backfill requires { "JOURNAL RAW SECONDS" }

m6a6f75726e616c2e636c692e64657363726962652e75736167652e6261636b66696c6c2e627564676574 = usage: { "solstone journal describe [-h] [--frames-only] [--redo] [-j N] [--journal PATH] [-v] [-d] FILE" }{ "\u000a" }{ "solstone journal describe" }: error: face backfill budget is outside the allowed range

m6a6f75726e616c2e636c692e64657363726962652e75736167652e6261636b66696c6c2e6465736b746f70 = usage: { "solstone journal describe [-h] [--frames-only] [--redo] [-j N] [--journal PATH] [-v] [-d] FILE" }{ "\u000a" }{ "solstone journal describe" }: error: face backfill is available on desktop journals only

m6a6f75726e616c2e636c692e64657363726962652e75736167652e64657363726962652e7265706561746564 = usage: { "solstone journal describe [-h] [--frames-only] [--redo] [-j N] [--journal PATH] [-v] [-d] FILE" }{ "\u000a" }{ "solstone journal describe" }: error: { "--describe" } was provided more than once

m6a6f75726e616c2e636c692e64657363726962652e75736167652e66696c652e6578747261 = usage: { "solstone journal describe [-h] [--frames-only] [--redo] [-j N] [--journal PATH] [-v] [-d] FILE" }{ "\u000a" }{ "solstone journal describe" }: error: only one video path may be provided

m6a6f75726e616c2e636c692e64657363726962652e75736167652e66696c652e7265717569726564 = usage: { "solstone journal describe [-h] [--frames-only] [--redo] [-j N] [--journal PATH] [-v] [-d] FILE" }{ "\u000a" }{ "solstone journal describe" }: error: the following arguments are required: { "FILE" }

m6a6f75726e616c2e636c692e64657363726962652e75736167652e6672616d65732e7265706561746564 = usage: { "solstone journal describe [-h] [--frames-only] [--redo] [-j N] [--journal PATH] [-v] [-d] FILE" }{ "\u000a" }{ "solstone journal describe" }: error: { "--frames-only" } was provided more than once

m6a6f75726e616c2e636c692e64657363726962652e75736167652e68656c70 = usage: { "solstone journal describe [-h] [--frames-only] [--redo] [-j N] [--journal PATH] [-v] [-d] FILE" }

m6a6f75726e616c2e636c692e64657363726962652e75736167652e68656c702e6578747261 = usage: { "solstone journal describe [-h] [--frames-only] [--redo] [-j N] [--journal PATH] [-v] [-d] FILE" }{ "\u000a" }{ "solstone journal describe" }: error: { "--help" } does not accept other arguments

m6a6f75726e616c2e636c692e64657363726962652e75736167652e6a6f62732e706f736974697665 = usage: { "solstone journal describe [-h] [--frames-only] [--redo] [-j N] [--journal PATH] [-v] [-d] FILE" }{ "\u000a" }{ "solstone journal describe" }: error: { "--jobs" } requires a positive integer

m6a6f75726e616c2e636c692e64657363726962652e75736167652e6a6f62732e7265706561746564 = usage: { "solstone journal describe [-h] [--frames-only] [--redo] [-j N] [--journal PATH] [-v] [-d] FILE" }{ "\u000a" }{ "solstone journal describe" }: error: { "--jobs" } was provided more than once

m6a6f75726e616c2e636c692e64657363726962652e75736167652e6a6f75726e616c2e70617468 = usage: { "solstone journal describe [-h] [--frames-only] [--redo] [-j N] [--journal PATH] [-v] [-d] FILE" }{ "\u000a" }{ "solstone journal describe" }: error: { "--journal" } requires a path

m6a6f75726e616c2e636c692e64657363726962652e75736167652e6a6f75726e616c2e7265706561746564 = usage: { "solstone journal describe [-h] [--frames-only] [--redo] [-j N] [--journal PATH] [-v] [-d] FILE" }{ "\u000a" }{ "solstone journal describe" }: error: { "--journal" } was provided more than once

m6a6f75726e616c2e636c692e64657363726962652e75736167652e6d6f64652e7265717569726564 = usage: { "solstone journal describe [-h] [--frames-only] [--redo] [-j N] [--journal PATH] [-v] [-d] FILE" }{ "\u000a" }{ "solstone journal describe" }: error: exactly one of { "--frames-only" } or { "--describe" } is required

m6a6f75726e616c2e636c692e64657363726962652e75736167652e7265646f2e6d6f6465 = usage: { "solstone journal describe [-h] [--frames-only] [--redo] [-j N] [--journal PATH] [-v] [-d] FILE" }{ "\u000a" }{ "solstone journal describe" }: error: { "--redo" } requires { "--describe" }

m6a6f75726e616c2e636c692e64657363726962652e75736167652e7265646f2e7265706561746564 = usage: { "solstone journal describe [-h] [--frames-only] [--redo] [-j N] [--journal PATH] [-v] [-d] FILE" }{ "\u000a" }{ "solstone journal describe" }: error: { "--redo" } was provided more than once

m6a6f75726e616c2e636c692e64657363726962652e75736167652e72656769737472792e6578747261 = usage: { "solstone journal describe [-h] [--frames-only] [--redo] [-j N] [--journal PATH] [-v] [-d] FILE" }{ "\u000a" }{ "solstone journal describe" }: error: { "--category-registry" } does not accept other arguments

m6a6f75726e616c2e636c692e64657363726962652e75736167652e76657273696f6e2e6578747261 = usage: { "solstone journal describe [-h] [--frames-only] [--redo] [-j N] [--journal PATH] [-v] [-d] FILE" }{ "\u000a" }{ "solstone journal describe" }: error: { "--version" } does not accept other arguments

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e62616e64 = { $i18nCategory ->
    [one] band { $band }: { $recordings } recording ready / { $recordings_left } left, { $outputs } daily { $i18nCategory-output_count ->
        [one] output
       *[other] outputs
    } ready / { $outputs_left } left, { $activities } { $i18nCategory-activity_count ->
        [one] activity
       *[other] activities
    } left, { $requests } owner { $i18nCategory-request_count ->
        [one] request
       *[other] requests
    }, { $permission } waiting for permission, { $prerequisites } waiting for prerequisites
   *[other] band { $band }: { $recordings } recordings ready / { $recordings_left } left, { $outputs } daily { $i18nCategory-output_count ->
        [one] output
       *[other] outputs
    } ready / { $outputs_left } left, { $activities } { $i18nCategory-activity_count ->
        [one] activity
       *[other] activities
    } left, { $requests } owner { $i18nCategory-request_count ->
        [one] request
       *[other] requests
    }, { $permission } waiting for permission, { $prerequisites } waiting for prerequisites
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e636175676874 = caught up

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e6361756768742e636170706564 = { $i18nCategory ->
    [one] caught up; { $days } day completed with { $i18nCategory-unit_count ->
        [one] a capped daily unit
       *[other] capped daily unit(s)
    }
   *[other] caught up; { $days } day(s) completed with { $i18nCategory-unit_count ->
        [one] a capped daily unit
       *[other] capped daily unit(s)
    }
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e6368616e67652e7265636f72646564 = { $name } recorded a change on { $location }; resolve the recorded change before that day is redone

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e6368616e67652e73746172746564 = { $name } may have started a change but did not confirm completion on { $location }; resolve the in-progress change before reprocessing

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e636865636b6564 = recent work checked

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e636865636b65642e636170706564 = { $i18nCategory ->
    [one] recent work checked; { $days } day completed with { $i18nCategory-unit_count ->
        [one] a capped daily unit
       *[other] capped daily unit(s)
    }
   *[other] recent work checked; { $days } day(s) completed with { $i18nCategory-unit_count ->
        [one] a capped daily unit
       *[other] capped daily unit(s)
    }
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e6465666572726564 = { $name } stopped on { $location }; it can retry on a later run after the local day changes

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e64656665727265642e726561736f6e = { $name } stopped on { $location } with reason { $reason }; it can retry on a later run after the local day changes

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e656c696769626c65 = { $name } stopped on { $location }; it can retry on the next eligible run

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e656c696769626c652e726561736f6e = { $name } stopped on { $location } with reason { $reason }; it can retry on the next eligible run

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e656d7074792e64617465 = none

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e657868617573746564 = { $name } stopped on { $location } after its automatic retry; run { $command }

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e6578686175737465642e726561736f6e = { $name } stopped on { $location } with reason { $reason } after its automatic retry; run { $command }

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e686f6c642e7265747279 = { $day }: { $reason } (retry { $unit })

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e696e76656e746f72792e7061727469616c2e63757272656e74 = { $i18nCategory ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection; set aside: { $holds }
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection; set aside: { $holds }
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e696e76656e746f72792e7061727469616c2e6c617374 = { $i18nCategory ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection; last known progress; set aside: { $holds }
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; incomplete inspection; last known progress; set aside: { $holds }
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e696e76656e746f72792e77686f6c652e63757272656e74 = { $i18nCategory ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; set aside: { $holds }
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; set aside: { $holds }
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e696e76656e746f72792e77686f6c652e6c617374 = { $i18nCategory ->
    [one] inspected { $inspected } of { $total } day, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; last known progress; set aside: { $holds }
   *[other] inspected { $inspected } of { $total } days, { $oldest } through { $newest }, as of { $as_of }; { $bands }; whole-journal inspection; last known progress; set aside: { $holds }
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e70656e64696e67 = pending: { $days }

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e70656e64696e672e636f756e7473 = { $i18nCategory ->
    [one] { $pending } day pending, { $stuck } { $i18nCategory-stuck_count ->
        [one] day
       *[other] day(s)
    } stuck
   *[other] { $pending } day(s) pending, { $stuck } { $i18nCategory-stuck_count ->
        [one] day
       *[other] day(s)
    } stuck
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e70656e64696e672e6f6c64657374 = { $i18nCategory ->
    [one] { $pending } day pending, { $stuck } { $i18nCategory-stuck_count ->
        [one] day
       *[other] day(s)
    } stuck; oldest outstanding { $day }
   *[other] { $pending } day(s) pending, { $stuck } { $i18nCategory-stuck_count ->
        [one] day
       *[other] day(s)
    } stuck; oldest outstanding { $day }
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e7265736f6c7665 = { $name } stopped on { $location }; resolve the conflict before reprocessing a past day

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e7265736f6c76652e726561736f6e = { $name } stopped on { $location } with reason { $reason }; resolve the conflict before reprocessing a past day

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e7265747279 = { $name } stopped on { $location }; run { $command }

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e72657472792e726561736f6e = { $name } stopped on { $location } with reason { $reason }; run { $command }

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e7265766965772e73746172746564 = { $name } may have started an entity review change but did not confirm completion on { $location }; resolve the in-progress change before reprocessing

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e737475636b = stuck: { $days }

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e756e66696e6973686564 = { $i18nCategory ->
    [one] { $activities } activity on { $days } completed { $i18nCategory-day_count ->
        [one] day
       *[other] days
    } couldn't finish processing
   *[other] { $activities } activities on { $days } completed { $i18nCategory-day_count ->
        [one] day
       *[other] days
    } couldn't finish processing
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e756e6b6e6f776e = unknown: { $days }

m6a6f75726e616c2e636c692e646f63746f722e6261636b6c6f672e756e6b6e6f776e2e636f756e74 = { $i18nCategory ->
    [one] couldn't fully determine; { $days } day unknown
   *[other] couldn't fully determine; { $days } day(s) unknown
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e6173736f63696174696f6e_v756e626f756e64 = setup association unknown; history only

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e6173736f63696174696f6e_v756e6b6e6f776e = current setup association unavailable

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e6173736f63696174696f6e_v73757065727365646564 = superseded setup; history only

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e6173736f63696174696f6e_v63757272656e74 = matches the selected setup

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e6173736f63696174696f6e_v696e76616c6964 = recorded setup association unavailable

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e696e636f6d706c657465_v73756363657373 = backup on; latest recorded attempt: successful copy at { $date }; some files were not copied; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e696e636f6d706c657465_v696e636f6d706c657465 = backup on; latest recorded attempt: incomplete copy at { $date }; some files were not copied; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e696e636f6d706c657465_v6661696c6564 = backup on; latest recorded attempt: failed at { $date }; some files were not copied; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e6c6f636b6564_v73756363657373 = backup on; latest recorded attempt: successful copy at { $date }; repository was locked; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e6c6f636b6564_v696e636f6d706c657465 = backup on; latest recorded attempt: incomplete copy at { $date }; repository was locked; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e6c6f636b6564_v6661696c6564 = backup on; latest recorded attempt: failed at { $date }; repository was locked; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e6f7264696e617279_v73756363657373 = backup on; latest recorded attempt: successful copy at { $date }; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e6f7264696e617279_v696e636f6d706c657465 = backup on; latest recorded attempt: incomplete copy at { $date }; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e6f7264696e617279_v6661696c6564 = backup on; latest recorded attempt: failed at { $date }; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e72656675736564_v73756363657373 = backup on; latest recorded attempt: successful copy at { $date }; backup key refused; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e72656675736564_v696e636f6d706c657465 = backup on; latest recorded attempt: incomplete copy at { $date }; backup key refused; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e72656675736564_v6661696c6564 = backup on; latest recorded attempt: failed at { $date }; backup key refused; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e74696d656f7574_v73756363657373 = backup on; latest recorded attempt: successful copy at { $date }; timed out; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e74696d656f7574_v696e636f6d706c657465 = backup on; latest recorded attempt: incomplete copy at { $date }; timed out; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e74696d656f7574_v6661696c6564 = backup on; latest recorded attempt: failed at { $date }; timed out; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e756e6b6e6f776e_v73756363657373 = backup on; latest recorded attempt: successful copy at { $date }; cause unknown; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e756e6b6e6f776e_v696e636f6d706c657465 = backup on; latest recorded attempt: incomplete copy at { $date }; cause unknown; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e617474656d70742e756e6b6e6f776e_v6661696c6564 = backup on; latest recorded attempt: failed at { $date }; cause unknown; { $history }; { $association }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e646174652e756e6b6e6f776e = unknown date

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e64657461696c73 = open backup or health in your journal for details

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e686973746f72792e696e636f6d706c6574652e6461746564 = { $i18nCategory ->
    [one] last successful copy (historical): snapshot { $snapshot } at { $date }, age { $seconds } second; incomplete copy, some files were not copied; present coverage and verification unknown
   *[other] last successful copy (historical): snapshot { $snapshot } at { $date }, age { $seconds } seconds; incomplete copy, some files were not copied; present coverage and verification unknown
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e686973746f72792e696e636f6d706c6574652e756e6461746564 = last successful copy (historical): snapshot { $snapshot } at { $date }, age unknown; incomplete copy, some files were not copied; present coverage and verification unknown

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e686973746f72792e737563636573732e6461746564 = { $i18nCategory ->
    [one] last successful copy (historical): snapshot { $snapshot } at { $date }, age { $seconds } second; present coverage and verification unknown
   *[other] last successful copy (historical): snapshot { $snapshot } at { $date }, age { $seconds } seconds; present coverage and verification unknown
}

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e686973746f72792e737563636573732e756e6461746564 = last successful copy (historical): snapshot { $snapshot } at { $date }, age unknown; present coverage and verification unknown

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e686973746f72792e756e6b6e6f776e = last successful copy unknown

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e696e76616c69642e686973746f7279 = backup on; recorded status unavailable; { $history }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e7061757365642e686973746f7279 = backup paused by you since { $date }; { $history }

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e736e617073686f742e756e6b6e6f776e = unknown

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e7374617475732e696e76616c69642e7061757365 = backup status unavailable: invalid pause setting

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e7374617475732e696e76616c69642e73657474696e6773 = backup status unavailable: invalid backup settings

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e7374617475732e696e76616c69642e737769746368 = backup status unavailable: invalid on/off setting

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e7374617475732e6e6f2e617474656d7074 = backup on; no recorded attempt

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e7374617475732e6f6666 = backup off

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e7374617475732e756e617661696c61626c652e6a6f75726e616c = backup status unavailable: journal unreadable

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e7374617475732e756e617661696c61626c652e73657474696e6773 = backup status unavailable: settings unreadable

m6a6f75726e616c2e636c692e646f63746f722e6261636b75702e7374617475732e756e636f6e66696775726564 = backup not configured

m6a6f75726e616c2e636c692e646f63746f722e627261696e2e6167652e756e6b6e6f776e = unknown

m6a6f75726e616c2e636c692e646f63746f722e627261696e2e6e6f2e636f6d706f6e656e74 = none

m6a6f75726e616c2e636c692e646f63746f722e627261696e2e7265706f7274 = { $headline }; { "state" }={ $state }; { "reason" }={ $reason }; { "component" }={ $component }; { "evidence_age" }={ $age }

m6a6f75726e616c2e636c692e646f63746f722e627261696e2e756e6b6e6f776e = unknown: brain state unavailable

m6a6f75726e616c2e636c692e646f63746f722e627261696e2e756e6b6e6f776e2e726561736f6e = unknown: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e6865616c74682e616374697665 = { "rollup=active" }; the { "solstone" } app on every device that has added to your journal is current

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e6865616c74682e646174612e756e617661696c61626c65 = { "rollup=unknown" }; device activity unavailable

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e6865616c74682e696e7370656374 = open { "/app/health" } to inspect each device

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e6865616c74682e6e6f2e73656e64657273 = { "rollup=no_senders" }; the { "solstone" } app hasn't added anything to your journal yet

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e6865616c74682e7175696574 = { "rollup=quiet" }; no rejected uploads recorded; see devices for last delivery times

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e6865616c74682e72656769737472792e756e617661696c61626c65 = { "rollup=unknown" }; device list unavailable

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e726563656e74 = the { "solstone" } app on { $client } last added { $hours }h ago

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e726563656e742e736f7572636573 = { $i18nCategory ->
    [one] the { "solstone" } app on { $client } last added { $hours }h ago ({ $sources } needs attention)
   *[other] the { "solstone" } app on { $client } last added { $hours }h ago ({ $sources } need attention)
}

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e726f6c6c7570 = { "rollup=attention" }; { $devices }

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e736f7572636573 = the { "solstone" } app on { $client } is having trouble adding { $sources }

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e74726f75626c65 = the { "solstone" } app on { $client } is having trouble adding

m6a6f75726e616c2e636c692e646f63746f722e636170747572652e74726f75626c652e736f7572636573 = { $i18nCategory ->
    [one] the { "solstone" } app on { $client } is having trouble adding ({ $sources } needs attention)
   *[other] the { "solstone" } app on { $client } is having trouble adding ({ $sources } need attention)
}

m6a6f75726e616c2e636c692e646f63746f722e636865636b2e6465666572726564 = deferred check set: { $set }

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e62696e64696e672e616374697665 = active client records={ $records }; unbound=0; { "certificate cid" } binding is authoritative

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e64656c69766572792e7374616c6c2e61637469766974792e756e617661696c61626c65 = device activity unavailable

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e64656c69766572792e7374616c6c2e696e7370656374 = open { "/app/health" } to inspect the rejected upload

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e64656c69766572792e7374616c6c2e6c6973742e756e617661696c61626c65 = device list unavailable

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e64656c69766572792e7374616c6c2e6e6f2e73656e64657273 = the { "solstone" } app hasn't added anything to your journal yet

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e64656c69766572792e7374616c6c2e7175696574 = no rejected uploads recorded; see devices for last delivery times

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e696e676573742e6865616c74682e6865616c746879 = no devices failing ingest

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e696e676573742e6865616c74682e72656a656374696f6e = device { $device } failing ingest: { $reason }, { $attempts }x since { $since }

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e696e676573742e6865616c74682e726570616972 = update or restart the device, then confirm a valid upload clears the rejection

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e7472616e73706f72742e7265667573616c2e6865616c746879 = no devices had requests turned away in the last 7 days

m6a6f75726e616c2e636c692e646f63746f722e636c69656e742e7472616e73706f72742e7265667573616c2e726570616972 = this is a busy connection, not a lost one; report it if it keeps happening

m6a6f75726e616c2e636c692e646f63746f722e636c69656e74732e6c65646765722e756e617661696c61626c65 = authorized client ledger unavailable: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e636f6e6669672e6469722e7265616461626c652e636f6e6669672e756e7772697461626c65 = service config directory is not writable: { $path }

m6a6f75726e616c2e636c692e646f63746f722e636f6e6669672e6469722e7265616461626c652e636f6e6669672e77696c6c2e637265617465 = home is writable; install will create { $path }

m6a6f75726e616c2e636c692e646f63746f722e636f6e6669672e6469722e7265616461626c652e636f6e6669672e7772697461626c65 = home and service config dir are writable ({ $path })

m6a6f75726e616c2e636c692e646f63746f722e636f6e6669672e6469722e7265616461626c652e686f6d652e6d697373696e67 = home directory does not exist: { $path }

m6a6f75726e616c2e636c692e646f63746f722e636f6e6669672e6469722e7265616461626c652e686f6d652e756e757361626c65 = home directory is not readable and writable: { $path }

m6a6f75726e616c2e636c692e646f63746f722e636f6e6669672e6469722e7265616461626c652e6f776e65722e756e617661696c61626c65 = could not locate the owner base: { $error }

m6a6f75726e616c2e636c692e646f63746f722e636f6e6669672e6469722e7265616461626c652e6f776e65722e77696e646f7773 = the Windows owner base is only readable on Windows

m6a6f75726e616c2e636c692e646f63746f722e636f6e6669672e726573746f7265 = repair or restore { "config/journal.json" } from a backup

m6a6f75726e616c2e636c692e646f63746f722e636f6e746578742e6661696c6564 = doctor failed to resolve context: { $error }

m6a6f75726e616c2e636c692e646f63746f722e64656661756c742e7374742e72656164792e6261636b656e64 = configured backend is { $backend }; parakeet readiness not applicable

m6a6f75726e616c2e636c692e646f63746f722e64656661756c742e7374742e72656164792e646f776e6c6f6164 = { "CoreML" } parakeet model is not downloaded — fetch it with: { "solstone journal install-models" }

m6a6f75726e616c2e636c692e646f63746f722e64656661756c742e7374742e72656164792e6f746865722e6d6f64656c2e7265616479 = parakeet model ready at { $path }

m6a6f75726e616c2e636c692e646f63746f722e64656661756c742e7374742e72656164792e756e737570706f72746564 = parakeet not supported on this platform

m6a6f75726e616c2e636c692e646f63746f722e64656c69766572792e72656a6563746564 = the journal rejected an upload from { $client }

m6a6f75726e616c2e636c692e646f63746f722e64656c69766572792e72656a65637465642e736f7572636573 = { $i18nCategory ->
    [one] the journal rejected an upload from { $client } ({ $sources } needs attention)
   *[other] the journal rejected an upload from { $client } ({ $sources } need attention)
}

m6a6f75726e616c2e636c692e646f63746f722e6465766963652e6461792e6c697374696e672e6661756c74 = day { $day } stream { $stream } device { $device }: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e6465766963652e6461792e6c697374696e672e7265616461626c65 = the journal can read the last { $days } days for every bound device

m6a6f75726e616c2e636c692e646f63746f722e6465766963652e6461792e6c697374696e672e726570616972 = a device syncing that day is told the journal cannot read it; a { "solstone" } app older than the 2026-09 fix shows that as offline; inspect the named stream directory's segments and their { "events.jsonl" }

m6a6f75726e616c2e636c692e646f63746f722e6465766963652e6461792e6c697374696e672e73747265616d732e756e617661696c61626c65 = device streams unavailable: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6465766963652e6461792e6c697374696e672e73747265616d732e756e626f756e64 = no device streams bound

m6a6f75726e616c2e636c692e646f63746f722e6465766963652e7265667573616c2e636f756e74 = { $i18nCategory ->
    [one] device { $device } had a request turned away: { $reason }, { $attempts }x through { $last_seen }
   *[other] device { $device } had requests turned away: { $reason }, { $attempts }x through { $last_seen }
}

m6a6f75726e616c2e636c692e646f63746f722e646576696365732e616273656e74 = no registered devices

m6a6f75726e616c2e636c692e646f63746f722e646576696365732e6d6f7265 = { $devices }{ $separator }+{ $extra } more

m6a6f75726e616c2e636c692e646f63746f722e646576696365732e756e617661696c61626c65 = device records unavailable: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6469736b2e73706163652e6c61796f75742e756e737570706f72746564 = resolved installation root is not a layout install tree

m6a6f75726e616c2e636c692e646f63746f722e6469736b2e73706163652e7061636b6167652e696e7374616c6c = a package install is replaced in place when it updates, so no second copy is needed

m6a6f75726e616c2e636c692e646f63746f722e6469736b2e73706163652e726f6f742e756e7265736f6c766564 = could not resolve installation root from install bin directory: { $path }

m6a6f75726e616c2e636c692e646f63746f722e6469736b2e73706163652e73697a652e6f766572666c6f77 = installation tree size overflow at { $path }

m6a6f75726e616c2e636c692e646f63746f722e6469736b2e73706163652e73697a652e756e7265616461626c65 = could not measure installation tree at { $path }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6469736b2e73706163652e73706163652e617661696c61626c65 = { $free_gib } { "GiB" } free on the filesystem holding { $path } ({ $required_gib } { "GiB" } is a second copy of this install plus headroom)

m6a6f75726e616c2e636c692e646f63746f722e6469736b2e73706163652e73706163652e696e7370656374 = could not inspect free space at { $path }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6469736b2e73706163652e73706163652e6c6f77 = only { $free_gib } { "GiB" } free on the filesystem holding { $path }; { $required_gib } { "GiB" } is a second copy of this install plus headroom

m6a6f75726e616c2e636c692e646f63746f722e6469736b2e73706163652e73706163652e756e7265616461626c65 = free space is not readable at { $path }

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e616374696f6e2e7265636f7665726564 = no action is needed: recovered records stay beside the originals and are listed here

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e616374696f6e2e72656675736564 = the journal will not start until this clears; each line above says whether it clears after the computer restarts or needs help

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e616374696f6e2e756e7265616461626c65 = check the paths under 'could not inspect'; any recovered records stay beside the originals and are listed here

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e68656c706572_v657869746564 = generation { $generation } admission { $admission }: helper { $identity }, exited

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e68656c706572_v657363616c61746564 = generation { $generation } admission { $admission }: helper { $identity }, retired (escalated: true)

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e68656c706572_v72657469726564 = generation { $generation } admission { $admission }: helper { $identity }, retired

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e68656c706572_v756e61636b6e6f776c6564676564 = generation { $generation } admission { $admission }: helper { $identity }, never-acknowledged

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e68656c706572_v737061776e = generation { $generation } admission { $admission }: helper { $identity }, spawn-failed

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e68656c706572_v756e6964656e746966696564 = generation { $generation } admission { $admission }: helper { $identity }, unidentified

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e68656c706572_v72656a6563746564 = generation { $generation } admission { $admission }: helper { $identity }, rejected-and-reaped

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e68656c706572_v726575736564 = generation { $generation } admission { $admission }: helper { $identity }, pid-reused-by-another-user

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e73657276696365_v657869746564 = generation { $generation } admission { $admission }: service { $identity }, exited

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e73657276696365_v657363616c61746564 = generation { $generation } admission { $admission }: service { $identity }, retired (escalated: true)

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e73657276696365_v72657469726564 = generation { $generation } admission { $admission }: service { $identity }, retired

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e73657276696365_v756e61636b6e6f776c6564676564 = generation { $generation } admission { $admission }: service { $identity }, never-acknowledged

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e73657276696365_v737061776e = generation { $generation } admission { $admission }: service { $identity }, spawn-failed

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e73657276696365_v756e6964656e746966696564 = generation { $generation } admission { $admission }: service { $identity }, unidentified

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e73657276696365_v72656a6563746564 = generation { $generation } admission { $admission }: service { $identity }, rejected-and-reaped

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e61646d697373696f6e2e73657276696365_v726575736564 = generation { $generation } admission { $admission }: service { $identity }, pid-reused-by-another-user

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e636c65616e = no recorded recovery to report

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e636c65616e2e6f75747374616e64696e67 = { $i18nCategory ->
    [one] no recorded recovery to report; the newest run (generation { $generation }) still owes { $launches } launch
   *[other] no recorded recovery to report; the newest run (generation { $generation }) still owes { $launches } launches
}

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e636c6f736564 = { $i18nCategory ->
    [one] generation { $generation } was closed by the start that became generation { $successor } (unix { $time }): { $total } admission, { $exited } already exited, { $retired } stopped by the journal, { $never } never admitted
   *[other] generation { $generation } was closed by the start that became generation { $successor } (unix { $time }): { $total } admissions, { $exited } already exited, { $retired } stopped by the journal, { $never } never admitted
}

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e636c6f7365642e726573746172746564 = { $i18nCategory ->
    [one] generation { $generation } was closed by the start that became generation { $successor } (unix { $time }): { $total } admission, { $exited } already exited, { $retired } stopped by the journal, { $never } never admitted; the computer had restarted since
   *[other] generation { $generation } was closed by the start that became generation { $successor } (unix { $time }): { $total } admissions, { $exited } already exited, { $retired } stopped by the journal, { $never } never admitted; the computer had restarted since
}

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e68656164696e672e6173696465 = bookkeeping set aside ({ $count }):

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e68656164696e672e636c6f736564 = runs closed by a later start ({ $count }):

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e68656164696e672e72656675736564 = starts refused ({ $count }):

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e68656164696e672e756e7265616461626c65 = could not inspect ({ $count }):

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e68656164696e672e756e7265736f6c766564 = runs that did not stop cleanly ({ $count }):

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e6d6f7265 = ... and { $count } more

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e6f75747374616e64696e67 = { $i18nCategory ->
    [one] the newest run (generation { $generation }) still owes { $launches } launch
   *[other] the newest run (generation { $generation }) still owes { $launches } launches
}

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e7265667573616c2e72657374617274 = generation { $generation }: the record of processes a retirement started is lost, so the journal will not start; it clears after the computer restarts

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e7265667573616c2e737570706f7274 = generation { $generation }: the record of processes a retirement started is lost, so the journal will not start; it will not clear on its own and needs help from support

m6a6f75726e616c2e636c692e646f63746f722e6475726162696c6974792e756e7265736f6c766564 = generation { $generation }: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e65786563757461626c652e6469726563746f7279 = could not determine executable directory: { $path }

m6a6f75726e616c2e636c692e646f63746f722e657865637574696f6e2e6572726f72 = check execution failed: { $kind }

m6a6f75726e616c2e636c692e646f63746f722e657865637574696f6e2e6572726f722e64657461696c = check execution failed: { $kind }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e66616365742e666978 = { $i18nCategory ->
    [one] that segment is missing from the activity lists; check the facet declarations with `{ "solstone journal facet doctor" }` and recent sense runs with `{ "solstone journal talent logs" }`
   *[other] those segments are missing from the activity lists; check the facet declarations with `{ "solstone journal facet doctor" }` and recent sense runs with `{ "solstone journal talent logs" }`
}

m6a6f75726e616c2e636c692e646f63746f722e66616365742e6e6f2e616374697665 = no active segments in the newest days

m6a6f75726e616c2e636c692e646f63746f722e66616365742e726f75746564 = { $i18nCategory ->
    [one] { $routed } of { $active } active segment since { $since } filed under a facet
   *[other] { $routed } of { $active } active segments since { $since } filed under a facet
}

m6a6f75726e616c2e636c692e646f63746f722e66616365742e756e726f75746564 = { $i18nCategory ->
    [one] { $unrouted } of { $active } active segment since { $since } { $i18nCategory-unrouted_quantity ->
        [one] was
       *[other] were
    } filed under no facet
   *[other] { $unrouted } of { $active } active segments since { $since } { $i18nCategory-unrouted_quantity ->
        [one] was
       *[other] were
    } filed under no facet
}

m6a6f75726e616c2e636c692e646f63746f722e68656c70 = usage: { "solstone journal doctor [-h] [--verbose] [--json] [--jsonl] [--port PORT]\u000a                      [--readiness]" }{ "\u000a" }{ "\u000a" }Run { "solstone" } diagnostics.{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  " }{ "-h, --help" }{ "         " }show this help message and exit{ "\u000a" }{ "  " }{ "--verbose" }{ "          " }print every check result{ "\u000a" }{ "  " }{ "--json" }{ "             " }emit JSON instead of text{ "\u000a" }{ "  " }{ "--jsonl" }{ "            " }emit one-JSON-per-line events instead of text{ "\u000a" }{ "  " }{ "--port PORT" }{ "        " }port to probe (default: { "5015" }){ "\u000a" }{ "  " }{ "--readiness" }{ "        " }run the setup readiness battery{ "\u000a" }{ "\u000a" }If '{ "solstone journal doctor" }' is unavailable, build the native doctor from this checkout:{ "\u000a" }{ "cargo run --manifest-path core/Cargo.toml -p solstone-core-doctor --bin solstone-core-doctor" }

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e706172746974696f6e = held partition { $partition }

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e706172746974696f6e2e73746f70706564 = partition { $partition } was running when the journal stopped; the journal holds it at its next start until its processes are gone

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e7265636f72642e656d707479 = record file is empty

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e7265636f72642e66696c656e616d65 = invalid partition file name in scope directory; the journal still reads it as a task record

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e7265636f72642e696e76616c6964 = cannot parse record: { $error }

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e7265636f72642e7374726179 = non-record entry in scope directory

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e7265636f72642e756e7265616461626c65 = cannot read record: { $error }

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e72656d6f7665 = remove { $path }: it holds nothing

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e73636f70652e756e6b6e6f776e = unrecognized scope directory format; the journal still holds any task recorded in it

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e73636f70652e756e7265616461626c65 = cannot list scope directory: { $error }

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e7374726179 = stray non-directory in task-queue in-flight path

m6a6f75726e616c2e636c692e646f63746f722e686f6c64732e756e7265616461626c65 = cannot list in-flight holds directory: { $error }

m6a6f75726e616c2e636c692e646f63746f722e696e6465782e6669782e636865636b696e67 = search is checking this itself and rebuilds if the damage is confirmed; run { "solstone journal indexer status" } for details

m6a6f75726e616c2e636c692e646f63746f722e696e6465782e6669782e64656c6574696f6e73 = search keeps retrying these removals; run { "solstone journal indexer status" } for details

m6a6f75726e616c2e636c692e646f63746f722e696e6465782e6669782e6572726f7273 = run { "solstone journal indexer --rescan" } and check its errors

m6a6f75726e616c2e636c692e646f63746f722e696e6465782e6669782e6661696c65642e66696c6573 = search retries these updates at its daily check; run { "solstone journal indexer status" } for details

m6a6f75726e616c2e636c692e646f63746f722e696e6465782e6669782e6661696c65642e776f726b = search retries this work every hour; run { "solstone journal indexer status" } for details

m6a6f75726e616c2e636c692e646f63746f722e696e6465782e6669782e66756c6c = run { "solstone journal indexer --rescan-full" }

m6a6f75726e616c2e636c692e646f63746f722e696e6465782e6669782e72656275696c64 = search rebuilds its index from your journal once the damage is confirmed; if it persists, run { "solstone journal indexer --reset --rescan-full" }

m6a6f75726e616c2e636c692e646f63746f722e696e6465782e6669782e7265736574 = run { "solstone journal indexer --reset --rescan-full" }

m6a6f75726e616c2e636c692e646f63746f722e696e6465782e6669782e75706772616465 = update { "solstone" } to the newest version

m6a6f75726e616c2e636c692e646f63746f722e696e6465782e6669782e7761726e696e6773 = run { "solstone journal indexer --rescan" } and check its warnings

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e616273656e74 = no local journal

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e6261636b6c6f672e756e617661696c61626c65 = couldn't fully determine; backlog read failed: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e6368726f6e69636c652e656e7472792e756e617661696c61626c65 = couldn't fully determine; failed to read chronicle entry: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e6368726f6e69636c652e756e617661696c61626c65 = couldn't fully determine; failed to read chronicle: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e636f6e666967732e756e617661696c61626c65 = couldn't fully determine; failed to read daily configs: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e636f7665726167652e756e617661696c61626c65 = couldn't determine daily unit coverage on { $day }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e646566696e6974696f6e732e756e617661696c61626c65 = couldn't fully determine; couldn't locate the daily processing definitions: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e6661636574732e6461792e756e617661696c61626c65 = couldn't fully determine; failed to check active facets for { $day }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e6661636574732e756e617661696c61626c65 = couldn't fully determine; failed to read facets: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e696e7370656374 = open health to see waiting work and any spending confirmation; reprocess a past day to give it priority after today

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e696e76656e746f72792e666978 = open health to see waiting work and its next step

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e696e76656e746f72792e756e617661696c61626c65 = whole-journal inspection unavailable: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e696e76656e746f72792e77616974696e67 = whole-journal inspection has not reported yet

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e6f7574736964652e6261636b6c6f67 = daily unit outside the recent backlog on { $day }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e726570616972 = re-run { "solstone journal doctor" }; check the health logs if it persists

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e756e69742e6964656e74697479 = couldn't read daily unit file { $file_name } on day { $day }: record identity does not match its path

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e756e69742e696e76616c6964 = couldn't fully determine; malformed daily unit file { $file_name } on day { $day }: { $source }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e756e69742e756e7265616461626c65 = couldn't fully determine; unreadable daily unit file { $file_name } on day { $day }: { $source }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e756e69742e76657273696f6e = couldn't read daily unit file { $file_name } on day { $day }: unsupported record version { $version }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e756e6974732e656e7472792e756e617661696c61626c65 = couldn't fully determine; failed to read daily-unit entry for { $day }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6361756768742e75702e756e6974732e756e617661696c61626c65 = couldn't fully determine; failed to read daily-units directory for { $day }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6469722e7772697461626c652e6d6f7665 = move or remove { $path }, then re-run

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6469722e7772697461626c652e6e6f742e6469726563746f7279 = journal path exists but is not a directory: { $path }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6469722e7772697461626c652e706172656e742e756e7772697461626c65 = journal dir absent; nearest existing ancestor is not writable: { $path }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6469722e7772697461626c652e706172656e742e7772697461626c65 = journal dir absent; parent { $path } is writable

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6469722e7772697461626c652e756e7772697461626c65 = journal dir not writable: { $path }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e6469722e7772697461626c652e7772697461626c65 = journal dir writable: { $path }

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e736f75726365732e7265616461626c652e6865616c746879 = the last scan found no unreadable days

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e736f75726365732e7265616461626c652e696e73706563742e666f6c64657273 = check the day folders named above — a name that is not a calendar date is not a day the journal can process

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e736f75726365732e7265616461626c652e7265706169722e66696c6573 = repair the file named above, then run { "solstone journal reprocess <day>" } for each listed day that has already ended

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e736f75726365732e7265616461626c652e726570726f63657373 = run { "solstone journal reprocess <day>" } for each listed day that has already ended

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e736f75726365732e7265616461626c652e73746174732e696e76616c6964 = couldn't check — the statistics aren't readable

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e736f75726365732e7265616461626c652e73746174732e6d697373696e67 = couldn't check — no statistics to read

m6a6f75726e616c2e636c692e646f63746f722e6a6f75726e616c2e736f75726365732e7265616461626c652e73746174732e7072656461746573 = couldn't check — the statistics predate this version

m6a6f75726e616c2e636c692e646f63746f722e6c61756e6368642e7374616c652e706c6973742e616273656e74 = launchd plist absent

m6a6f75726e616c2e636c692e646f63746f722e6c61756e6368642e7374616c652e706c6973742e617267756d656e74732e6d697373696e67 = plist is missing { "ProgramArguments[0]" }

m6a6f75726e616c2e636c692e646f63746f722e6c61756e6368642e7374616c652e706c6973742e696e76616c6964 = could not parse plist: { "plist::Error" }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e6c61756e6368642e7374616c652e706c6973742e726570616972 = run { "solstone journal service uninstall" }, then run { "solstone journal service install" } separately to reinstall a headless background service

m6a6f75726e616c2e636c692e646f63746f722e6c61756e6368642e7374616c652e706c6973742e7461726765742e657869737473 = launchd plist target exists ({ $path })

m6a6f75726e616c2e636c692e646f63746f722e6c61756e6368642e7374616c652e706c6973742e7461726765742e6d697373696e67 = plist points to missing executable: { $path }

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e66696c652e696e76616c6964 = { $path } is not a file

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e66696c652e6d697373696e67 = { $path } is missing

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e706174682e616273656e74 = { "solstone" } is not on { "PATH" }

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e706174682e6d69736d61746368 = { "PATH" } { "solstone" } resolves to { $resolved }, expected { $expected }

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e706174682e7265616479 = { "~/.local/bin/solstone" } is on { "PATH" } at { $path }

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e706174682e726570616972 = put { "~/.local/bin" } earlier on { "PATH" }, or run { "solstone journal setup" } to repoint the managed wrapper

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e73796d6c696e6b2e7265616479 = { "~/.local/bin/solstone" } symlinks to { "PATH" } { "solstone" } at { $path }

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e77696e646f77732e706174682e7072696f72697479 = put { $install_dir } ahead of { $other_dir } on your { "PATH" }

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e77696e646f77732e7265616479 = { "solstone" } is on { "PATH" } at { $path }

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e77696e646f77732e726570616972 = reinstall the journal to restore { "solstone.exe" }

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e77696e646f77732e7465726d696e616c = open a new terminal window and try again; if { "solstone" } is still not found, reinstall the journal

m6a6f75726e616c2e636c692e646f63746f722e6c6f63616c2e62696e2e736f6c73746f6e652e726561636861626c652e777261707065722e696e7374616c6c = run { "solstone journal setup" } to install the managed { "solstone" } wrapper

m6a6f75726e616c2e636c692e646f63746f722e6f727068616e2e7064662e666f756e64 = { $i18nCategory ->
    [one] { $count } raw PDF original without a readable document transcript: { $paths }
   *[other] { $count } raw PDF original(s) without a readable document transcript: { $paths }
}

m6a6f75726e616c2e636c692e646f63746f722e6f727068616e2e7365676d656e742e7064662e6368726f6e69636c652e756e617661696c61626c65 = chronicle directory unavailable

m6a6f75726e616c2e636c692e646f63746f722e6f727068616e2e7365676d656e742e7064662e636c656172 = all raw PDF originals have readable document transcripts

m6a6f75726e616c2e636c692e646f63746f722e6f727068616e2e7365676d656e742e7064662e726570616972 = add a readable { "*_transcript.md" } beside the PDF original, then re-run { "solstone journal doctor" }

m6a6f75726e616c2e636c692e646f63746f722e7061636b6167652e65786563757461626c652e706172656e74 = executable has no parent

m6a6f75726e616c2e636c692e646f63746f722e7061636b6167652e6c61796f7574 = could not locate packaged talent roots from executable directory { $directory }{ "\u000a" }walked ancestor { $ancestor }{ "\u000a" }site-packages candidate: { $site }{ "\u000a" }checkout candidate ({ "pyproject.toml" } + { ".git" }): { $checkout }{ "\u000a" }layout share anchors { $bundle }, { $layout }, { $template }: { $install }{ "\u000a" }Apple bundle { "JournalPayload" } candidate: { $embedded }

m6a6f75726e616c2e636c692e646f63746f722e7061636b6167652e6d697373696e67 = installation root { $root } is missing directory { $path }

m6a6f75726e616c2e636c692e646f63746f722e7061636b6167652e6e6f6e65 = none

m6a6f75726e616c2e636c692e646f63746f722e7061636b6167652e746172676574 = { "\u000a" }Cargo target directory { $target } is outside any checkout: a binary built there cannot find the payload, so keep { "CARGO_TARGET_DIR" } inside the checkout

m6a6f75726e616c2e636c692e646f63746f722e7061636b6167652e756e617661696c61626c65 = could not locate packaged talent roots from executable directory { $directory }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e636f72656d6c2e636f6e66696775726564 = parakeet check failed: installed cache { $installed } does not match configured { $configured }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e636f72656d6c2e646967657374 = parakeet check failed: asset digest mismatch for { $file }: expected { $expected }, found { $found }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e636f72656d6c2e6661696c6564 = parakeet check failed: { $error }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e636f72656d6c2e6d697373696e67 = parakeet check failed: asset missing: { $file }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e636f72656d6c2e6d697373696e672e6572726f72 = parakeet check failed: asset missing: { $file }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e636f72656d6c2e73656e74696e656c = parakeet check failed: sentinel not ready at { $path }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e636f72656d6c2e73697a65 = parakeet check failed: asset size mismatch for { $file }: expected { $expected }, found { $found }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e636f72656d6c2e756e7265616461626c65 = parakeet check failed: failed to read { $file }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e65786563757461626c65 = { "parakeet-cpp" } check failed: { $name } not executable at { $path }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e6d697373696e67 = { "parakeet-cpp" } check failed: { $name } missing at { $path }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e6d6f64656c = { "parakeet-cpp" } check failed: model missing at { $path }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e70696e = { "parakeet-cpp" } pin lacks { $key }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e706c6174666f726d = { "parakeet-cpp" } is unsupported on { $os }/{ $arch }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e6261636b656e642e6e6f742e73656c6563746564 = configured backend is not { "parakeet-cpp" }; check not applicable

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e62696e6172792e756e617661696c61626c65 = { "parakeet-cpp" } binary cannot start

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e696e7374616c6c = { "parakeet-cpp" } artifacts are not installed — fetch them with: { "solstone journal install-provider parakeet" }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e6f66666c696e65 = { "parakeet-server" } not reachable: your journal isn't running

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e6f6e6c696e65 = { "parakeet-cpp" } ready (server + model match the signed package, server reachable)

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e7265616479 = { "parakeet-cpp" } ready (binaries + model installed, server reachable)

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e72756e74696d652e696e7374616c6c = install the system { "OpenMP" } runtime that provides { "libgomp.so.1" }, then rerun { "solstone journal doctor" }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e72756e74696d652e756e617661696c61626c65 = { "parakeet-cpp" } cannot start: { "OpenMP" } runtime unavailable ({ "libgomp.so.1" })

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e7365727665722e6d697373696e67 = { "parakeet-server" } not reachable: your journal is running, but not { "parakeet-server" }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e7374617274 = { "parakeet-server" } is not reachable — start the journal service: { "solstone journal up" }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e756e726561636861626c65 = { "parakeet-server" } not reachable: { $error }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e6370702e7374742e72656164792e756e737570706f72746564 = { "parakeet-cpp" } is only supported on linux and windows

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e7068617365 = { "parakeet-server" } not reachable: your journal reports its state as { $phase }

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e70686173652e636865636b696e67 = { "parakeet-server" } not reachable: your journal reports its state as checking

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e7265696e7374616c6c = reinstall the journal

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e756e726561636861626c65_v616273656e74 = { "parakeet-server" } not reachable: nothing is listening

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e756e726561636861626c65_v756e6b6e6f776e = { "parakeet-server" } not reachable: can't tell whether anything is listening

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e756e726561636861626c65_v72756e74696d65 = { "parakeet-server" } not reachable: nothing answered

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e756e726561636861626c65_v74696d656f7574 = { "parakeet-server" } not reachable: took too long to answer

m6a6f75726e616c2e636c692e646f63746f722e706172616b6565742e756e726561636861626c65_v7472616e73706f7274 = { "parakeet-server" } not reachable: the connection dropped

m6a6f75726e616c2e636c692e646f63746f722e706174682e726570616972 = fix ownership/permissions of { $path }

m6a6f75726e616c2e636c692e646f63746f722e706c6174666f726d2e756e737570706f72746564 = not supported on { $platform }

m6a6f75726e616c2e636c692e646f63746f722e70726f67726573732e636865636b = { $seconds }s checking { $name }

m6a6f75726e616c2e636c692e646f63746f722e70726f67726573732e68617368 = { $seconds }s verifying { $member }: { $hashed } of { $total } { "MB" }

m6a6f75726e616c2e636c692e646f63746f722e7365617263682e6c6f672e66696e6973686564 = last finished search check could not be read: { $error }

m6a6f75726e616c2e636c692e646f63746f722e7365617263682e6c6f672e756e617661696c61626c65 = search index status unavailable: { $error }

m6a6f75726e616c2e636c692e646f63746f722e7365617263682e6c6f672e776f726b = search work list could not be read: { $error }

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e6361707065642e726561736f6e = at least 99 Sense dispatch errors since the last successful handler sync; latest reason: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e6361707065642e756e6b6e6f776e = at least 99 Sense dispatch errors since the last successful handler sync; latest reason unavailable

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e636c656172 = no Sense dispatch errors since the last successful handler sync

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e636f756e742e696e76616c6964 = Sense status is incomplete: no valid nonnegative dispatch error count

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e636f756e742e6f75742e6f662e72616e6765 = Sense status is incomplete: dispatch error count is outside its supported range

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e6572726f72732e726561736f6e = { $i18nCategory ->
    [one] { $count } Sense dispatch error since the last successful handler sync; latest reason: { $reason }
   *[other] { $count } Sense dispatch errors since the last successful handler sync; latest reason: { $reason }
}

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e6572726f72732e756e6b6e6f776e = { $i18nCategory ->
    [one] { $count } Sense dispatch error since the last successful handler sync; latest reason unavailable
   *[other] { $count } Sense dispatch errors since the last successful handler sync; latest reason unavailable
}

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e756e726561636861626c65_v616273656e74 = could not confirm Sense dispatch health: nothing is listening

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e756e726561636861626c65_v756e6b6e6f776e = could not confirm Sense dispatch health: can't tell whether anything is listening

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e756e726561636861626c65_v72756e74696d65 = could not confirm Sense dispatch health: nothing answered

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e756e726561636861626c65_v74696d656f7574 = could not confirm Sense dispatch health: took too long to answer

m6a6f75726e616c2e636c692e646f63746f722e73656e73652e64697370617463682e756e726561636861626c65_v7472616e73706f7274 = could not confirm Sense dispatch health: the connection dropped

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e616273656e74 = no local journal service

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e636f6e666c6963742e7761726e696e67 = resolve the macOS supervisor topology warning before changing the journal service

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e63726173682e6c697374 = crash-loop: { $services }

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e6964656e746974792e726570616972 = run { "solstone journal setup" } to reinstall the service

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e6964656e746974792e7265706169722e666f726365 = run { "solstone journal setup --force" } from this install to refresh the service

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e6964656e746974792e7461726765742e616273656e74 = service unit is malformed: no executable target in { $path }

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e6964656e746974792e7461726765742e63757272656e74 = service target matches current install: { $raw } -> { $path }

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e6964656e746974792e7461726765742e6d69736d61746368 = service target mismatch: { $raw } resolves to { $resolved }, expected { $expected }

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e6964656e746974792e77696e646f77732e63757272656e74 = service target matches current install: { $command }

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e6964656e746974792e77696e646f77732e6d69736d61746368 = service target mismatch: { $mismatch }

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e726567697374726174696f6e2e756e7265616461626c65 = couldn't read the journal's service registration — { $reason }

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e726573746172742e617474656d707473 = { $i18nCategory ->
    [one] { $name } ({ $attempts } restart attempt)
   *[other] { $name } ({ $attempts } restart attempts)
}

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e6c6f6773 = run { "solstone journal service logs" }

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e7069642e696e76616c6964 = probe child PID does not fit a process-group ID

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e726567697374726174696f6e2e6e6f2e726561646572 = no reader for the service registration

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e72657374617274 = run { "solstone journal service restart" }; if it persists, run { "solstone journal service logs" }

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e72756e6e696e67 = journal service is running

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e7374617274 = run { "solstone journal service start" }

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e73746f70706564 = service installed but not running

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e756e69742e6661696c6564 = journal service unit is failed

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e756e726561636861626c65_v616273656e74 = couldn't get a status — nothing is listening

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e756e726561636861626c65_v756e6b6e6f776e = couldn't get a status — can't tell whether anything is listening

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e756e726561636861626c65_v72756e74696d65 = couldn't get a status — nothing answered

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e756e726561636861626c65_v74696d656f7574 = couldn't get a status — took too long to answer

m6a6f75726e616c2e636c692e646f63746f722e736572766963652e72756e6e696e672e756e726561636861626c65_v7472616e73706f7274 = couldn't get a status — the connection dropped

m6a6f75726e616c2e636c692e646f63746f722e736b696c6c2e73746174652e636865636b6f75742e7265717569726564 = router skill state is only available from a source checkout

m6a6f75726e616c2e636c692e646f63746f722e736b696c6c2e73746174652e6469726563746f726965732e6d697373696e67 = router skill directories are unavailable

m6a6f75726e616c2e636c692e646f63746f722e736b696c6c2e73746174652e696e7374616c6c = run { "solstone skills install --project ." }

m6a6f75726e616c2e636c692e646f63746f722e736b696c6c2e73746174652e6c696e6b2e666f726569676e = { $skill } points elsewhere at { $path }

m6a6f75726e616c2e636c692e646f63746f722e736b696c6c2e73746174652e6c696e6b2e6d697373696e67 = { $skill } missing at { $path }

m6a6f75726e616c2e636c692e646f63746f722e736b696c6c2e73746174652e6c696e6b2e7374616c65 = stale router skill link at { $path }

m6a6f75726e616c2e636c692e646f63746f722e736b696c6c2e73746174652e6c696e6b732e7265616479 = router skills { "solstone" }, journal are installed and current

m6a6f75726e616c2e636c692e646f63746f722e736f757263652e64656661756c74 = default

m6a6f75726e616c2e636c692e646f63746f722e736f75726365732e646179732e6d6f7265 = { $days }, and { $more } more

m6a6f75726e616c2e636c692e646f63746f722e736f75726365732e666f6c642e6661696c6564 = { $i18nCategory ->
    [one] { $count } day didn't finish processing ({ $days })
   *[other] { $count } day(s) didn't finish processing ({ $days })
}

m6a6f75726e616c2e636c692e646f63746f722e736f75726365732e737461746973746963732e72656672657368 = run { "solstone journal journal-stats" }; check the health logs if it persists

m6a6f75726e616c2e636c692e646f63746f722e736f75726365732e737461746973746963732e7374616c65 = couldn't check — the statistics are { $hours } hours old

m6a6f75726e616c2e636c692e646f63746f722e736f75726365732e756e7265616461626c65 = { $i18nCategory ->
    [one] { $count } day couldn't be scanned ({ $days })
   *[other] { $count } day(s) couldn't be scanned ({ $days })
}

m6a6f75726e616c2e636c692e646f63746f722e736f75726365732e756e7265616461626c652e6361757365 = { $i18nCategory ->
    [one] { $count } day couldn't be scanned ({ $days }): { $day }: { $cause }
   *[other] { $count } day(s) couldn't be scanned ({ $days }): { $day }: { $cause }
}

m6a6f75726e616c2e636c692e646f63746f722e736f75726365732e756e7265616461626c652e756e6461746564 = { $i18nCategory ->
    [one] { $count } day couldn't be scanned ({ $days }): that day: { $cause }
   *[other] { $count } day(s) couldn't be scanned ({ $days }): that day: { $cause }
}

m6a6f75726e616c2e636c692e646f63746f722e737065616b6572732e696e7374616c6c6174696f6e2e617070 = { "Speakers-analyze" } installation is incomplete ({ $detail }). Repair: reinstall the journal app.

m6a6f75726e616c2e636c692e646f63746f722e737065616b6572732e696e7374616c6c6174696f6e2e6a6f75726e616c = { "Speakers-analyze" } installation is incomplete ({ $detail }). Repair: reinstall the journal.

m6a6f75726e616c2e636c692e646f63746f722e737065616b6572732e7265616479 = { "speakers-analyze" } installation ready

m6a6f75726e616c2e636c692e646f63746f722e737065616b6572732e7265696e7374616c6c2e617070 = reinstall the journal app

m6a6f75726e616c2e636c692e646f63746f722e737065616b6572732e756e617661696c61626c65 = { "speakers-analyze" } installation unavailable

m6a6f75726e616c2e636c692e646f63746f722e7374617475732e6572726f72 = ERROR

m6a6f75726e616c2e636c692e646f63746f722e7374617475732e6661696c = FAIL

m6a6f75726e616c2e636c692e646f63746f722e7374617475732e6f6b = OK

m6a6f75726e616c2e636c692e646f63746f722e7374617475732e736b6970 = SKIP

m6a6f75726e616c2e636c692e646f63746f722e7374617475732e7761726e = WARN

m6a6f75726e616c2e636c692e646f63746f722e73746f726167652e696e7370656374696f6e2e6c6f67 = storage privacy: { $error }

m6a6f75726e616c2e636c692e646f63746f722e73746f726167652e707269766163792e63686f696365 = other accounts on this computer may be able to read your journal. its folder could not be made private on this drive. you can keep it on this drive or move it to a drive that keeps folders private. to keep it on this drive, choose that on your journal's health page.

m6a6f75726e616c2e636c692e646f63746f722e73756d6d617279 = { $i18nCategory ->
    [one] doctor: { $total } check, { $failed } failed, { $warnings } { $i18nCategory-warning_quantity ->
        [one] warning
       *[other] warnings
    }, { $skipped } skipped, { $errors } { $i18nCategory-error_quantity ->
        [one] error
       *[other] errors
    }
   *[other] doctor: { $total } checks, { $failed } failed, { $warnings } { $i18nCategory-warning_quantity ->
        [one] warning
       *[other] warnings
    }, { $skipped } skipped, { $errors } { $i18nCategory-error_quantity ->
        [one] error
       *[other] errors
    }
}

m6a6f75726e616c2e636c692e646f63746f722e73757065727669736f722e636f6e666c6963742e636c656172 = no macOS supervisor conflict (native check: foreign-launcher scan only; no competing KeepAlive launcher found)

m6a6f75726e616c2e636c692e646f63746f722e73757065727669736f722e636f6e666c6963742e726570616972 = { $i18nCategory ->
    [one] remove the foreign launcher targeting { "/Applications/solstone.app" }: { $commands }; then rerun { "solstone journal doctor" }
   *[other] remove foreign launchers targeting { "/Applications/solstone.app" }: { $commands }; then rerun { "solstone journal doctor" }
}

m6a6f75726e616c2e636c692e646f63746f722e73757065727669736f722e636f6e666c6963742e7265706f7274 = { $i18nCategory ->
    [one] macOS supervisor conflict: { $count } foreign { "KeepAlive" } launcher targets { "/Applications/solstone.app" }
   *[other] macOS supervisor conflict: { $count } foreign { "KeepAlive" } launchers target { "/Applications/solstone.app" }
}

m6a6f75726e616c2e636c692e646f63746f722e73757065727669736f722e636f6e666c6963742e7265736f6c7665 = { $i18nCategory ->
    [one] resolve the macOS supervisor conflict first: remove the foreign launcher targeting { "/Applications/solstone.app" }: { $commands }; then rerun { "solstone journal doctor" }
   *[other] resolve the macOS supervisor conflict first: remove foreign launchers targeting { "/Applications/solstone.app" }: { $commands }; then rerun { "solstone journal doctor" }
}

m6a6f75726e616c2e636c692e646f63746f722e73757065727669736f722e636f6e666c6963742e756e737570706f72746564 = not supported on windows

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6469726563746f72792e7265717569726564 = flat-directory descendant is not a directory: { $path }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e656e7472792e6368616e676564 = entry changed during observation: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e656e7472792e696e76616c6964 = invalid flat-directory entry { $entry }: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e656e7472792e7265706c61636564 = changed or disappeared during inspection

m6a6f75726e616c2e636c692e646f63746f722e73796e632e656e7472792e756e73616665 = Installation: needs attention{ "\u000a" }your journal contains an item that can't be checked safely.{ "\u000a" }move the item named under details out of your journal's `{ "health/sync" }` folder, then try again.{ "\u000a" }{ "\u000a" }details:{ "\u000a" }path: { $path }{ "\u000a" }type: { $entry_type }{ "\u000a" }reason: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e656e7472792e756e736166652e6279746573 = Installation: needs attention{ "\u000a" }your journal contains an item that can't be checked safely.{ "\u000a" }move the item named under details out of your journal's `{ "health/sync" }` folder, then try again.{ "\u000a" }{ "\u000a" }details:{ "\u000a" }path: { $path }{ "\u000a" }type: { $entry_type }{ "\u000a" }reason: { $reason }{ "\u000a" }name bytes: { $bytes }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e656e7472792e76616e6973686564 = flat-directory entry vanished while listing: { $path }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e66696c652e7265717569726564 = flat-directory entry is not a regular file: { $path }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e666f6c6465722e66756c6c = Installation: needs attention{ "\u000a" }your journal has too many items to check safely.{ "\u000a" }review the items in your journal's `{ "health/sync" }` folder. move only items you know do not belong there until 256 or fewer remain, then try again.{ "\u000a" }{ "\u000a" }details:{ "\u000a" }folder: { $folder }{ "\u000a" }items found: more than 256{ "\u000a" }maximum checked: 256

m6a6f75726e616c2e636c692e646f63746f722e73796e632e666f6c6465722e756e73616665 = Installation: needs attention{ "\u000a" }part of your journal can't be checked safely.{ "\u000a" }check the path named under details. it needs to be an ordinary folder inside your journal that you can open before you try again.{ "\u000a" }{ "\u000a" }details:{ "\u000a" }path: { $path }{ "\u000a" }reason: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e686561727462656174 = Installation: needs attention{ "\u000a" }a recent heartbeat from another run is present.{ "\u000a" }wait a moment, then try again.

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6964656e746974792e6368616e676564 = flat-directory identity changed: { $path }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e696f = { $operation } failed for { $path }: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6b696e642e626c6f636b646576696365 = block device

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6b696e642e636861726163746572646576696365 = character device

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6b696e642e6469726563746f7279 = directory

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6b696e642e6669666f = named pipe

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6b696e642e6f74686572 = other filesystem object

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6b696e642e726567756c617266696c65 = regular file

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6b696e642e736f636b6574 = Unix socket

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6b696e642e73796d6c696e6b = symbolic link

m6a6f75726e616c2e636c692e646f63746f722e73796e632e6c6f63616c = this computer only ({ $host })

m6a6f75726e616c2e636c692e646f63746f722e73796e632e706174682e696e76616c6964 = invalid flat-directory path { $path }: { $reason }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e706565722e6c6567616379 = this computer only ({ $host }){ "\u000a" }  last other writer: { $writer } (legacy machine { $prefix }...)

m6a6f75726e616c2e636c692e646f63746f722e73796e632e706565722e756e6b6e6f776e = this computer only ({ $host }){ "\u000a" }  last other writer: { $writer } (unidentified heartbeat)

m6a6f75726e616c2e636c692e646f63746f722e73796e632e706565722e777269746572 = this computer only ({ $host }){ "\u000a" }  last other writer: { $writer } (writer { $prefix }... run { $run })

m6a6f75726e616c2e636c692e646f63746f722e73796e632e726561736f6e2e6469736170706561726564 = entry disappeared during observation

m6a6f75726e616c2e636c692e646f63746f722e73796e632e726561736f6e2e6e6f6e726567756c6172 = not a regular file ({ $kind })

m6a6f75726e616c2e636c692e646f63746f722e73796e632e726561736f6e2e6f76657273697a6564 = regular file is { $size } bytes, limit is { $limit }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e726561736f6e2e72656164 = could not read observed file

m6a6f75726e616c2e636c692e646f63746f722e73796e632e73697a65 = flat-directory entry exceeds observed-read limit: { $path } is { $kind }, { $size } bytes exceeds { $limit }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e73796d6c696e6b = flat-directory descendant is a symlink: { $path }

m6a6f75726e616c2e636c692e646f63746f722e73796e632e756e737570706f72746564 = Installation: needs attention{ "\u000a" }sync rescan capability is unavailable on this platform.

m6a6f75726e616c2e636c692e646f63746f722e73796e632e756e76657269666961626c65 = Installation: needs attention{ "\u000a" }startup status couldn't be verified.{ "\u000a" }wait a moment, then try again.

m6a6f75726e616c2e636c692e646f63746f722e73796e632e77616974696e67 = Installation: waiting{ "\u000a" }a recent heartbeat from another run was found.{ "\u000a" }the { "solstone" } app is waiting while it checks whether that activity continues. it will check again shortly.

m6a6f75726e616c2e636c692e646f63746f722e7461736b2e706163652e61626f75742e636170 = a job is running long; it will be stopped automatically if it passes its cap — no action needed unless it persists

m6a6f75726e616c2e636c692e646f63746f722e7461736b2e706163652e6865616c746879 = tasks on pace

m6a6f75726e616c2e636c692e646f63746f722e7461736b2e706163652e7374617475732e756e617661696c61626c65 = supervisor status unavailable

m6a6f75726e616c2e636c692e646f63746f722e7461736b2e71756575652e686f6c64732e616273656e74 = no in-flight or held task partition records present

m6a6f75726e616c2e636c692e646f63746f722e7461736b2e71756575652e686f6c64732e636c656172 = all task partition records are clean

m6a6f75726e616c2e636c692e646f63746f722e7461736b732e6475726174696f6e = { $task } ({ $seconds }s of { $limit }s cap)

m6a6f75726e616c2e636c692e646f63746f722e7461736b732e6c6f6e67 = running long: { $tasks }

m6a6f75726e616c2e636c692e646f63746f722e7472616e736372697074696f6e2e756e726574727961626c65 = { $i18nCategory ->
    [one] { $count } recording the journal will not retry on its own ({ "handler" }: { $handler }, { "reason_code" }: { $reason_code }): { $identities }
   *[other] { $count } recordings the journal will not retry on its own ({ "handler" }: { $handler }, { "reason_code" }: { $reason_code }): { $identities }
}

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e636c656172 = 0 recordings the journal will not retry on its own (handler: { $handler }, { "reason_code" }: { $reason_code })

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e646179732e756e7265616461626c65 = failed to read chronicle days: { $error }

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e6469726563746f72792e756e7265616461626c65 = cannot read segment dir { $path }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e656e7472792e756e7265616461626c65 = cannot read segment entry in { $path }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e68616e646c65722e6d697373696e67 = processing record in { $path } missing handler field

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e6964656e746974792e696e76616c6964 = invalid segment identity in { $path }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e6f70656e = cannot open { $path }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e72656164 = cannot read { $path }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e726f772e656e636f64696e67 = first row in { $path } is not valid utf-8: { $error }

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e726f772e6a736f6e = first row in { $path } is not valid json: { $error }

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e726f772e6c61726765 = first row in { $path } exceeds byte limit

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e726f772e756e617661696c61626c65 = first row in { $path } cannot be established or exceeds byte limit

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e7363616e2e6661696c6564 = could not determine how many recordings the journal will not retry on its own ({ $reason })

m6a6f75726e616c2e636c692e646f63746f722e756e726574727961626c652e7472616e7363726962652e696e7075742e7365676d656e74732e756e7265616461626c65 = failed to list segments for day { $day }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e75736167652e617267756d656e74 = usage: { "solstone journal doctor [-h] [--verbose] [--json] [--jsonl] [--port PORT]\u000a                      [--readiness]" }{ "\u000a" }{ "solstone journal doctor" }: error: unexpected argument

m6a6f75726e616c2e636c692e646f63746f722e75736167652e666f726d617473 = usage: { "solstone journal doctor [-h] [--verbose] [--json] [--jsonl] [--port PORT]\u000a                      [--readiness]" }{ "\u000a" }{ "solstone journal doctor" }: error: { "--json" } and { "--jsonl" } are mutually exclusive

m6a6f75726e616c2e636c692e646f63746f722e75736167652e706f7274 = usage: { "solstone journal doctor [-h] [--verbose] [--json] [--jsonl] [--port PORT]\u000a                      [--readiness]" }{ "\u000a" }{ "solstone journal doctor" }: error: { "--port" } requires an integer

m6a6f75726e616c2e636c692e646f63746f722e7661642e636f6e7472616374 = { "VAD" } helper ran but did not report closed-stdin usage contract (exit { $exit_code }): { $stderr }

m6a6f75726e616c2e636c692e646f63746f722e7661642e65786563757461626c65 = could not determine current executable for { "VAD" } helper: { $error }

m6a6f75726e616c2e636c692e646f63746f722e7661642e6c61796f7574 = { "VAD" } helper lookup is outside the running signed app payload

m6a6f75726e616c2e636c692e646f63746f722e7661642e6c6f61646572 = { "VAD" } helper could not load { $library }

m6a6f75726e616c2e636c692e646f63746f722e7661642e6d697373696e67 = { "VAD" } helper binary is missing: { $path }

m6a6f75726e616c2e636c692e646f63746f722e7661642e706172656e74 = current executable has no parent directory for { "VAD" } helper lookup

m6a6f75726e616c2e636c692e646f63746f722e7661642e70726f62652e6c6f67 = { "VAD" } helper probe failed: { $error }

m6a6f75726e616c2e636c692e646f63746f722e7661642e7265616479 = { "VAD" } helper launchable (closed-stdin usage contract)

m6a6f75726e616c2e636c692e646f63746f722e7661642e7265706169722e696e7374616c6c = reinstall the journal, then rerun { "solstone journal doctor" }

m6a6f75726e616c2e636c692e646f63746f722e7661642e7265706169722e696e7374616c6c2e617070 = reinstall the journal app, then rerun { "solstone journal doctor" }

m6a6f75726e616c2e636c692e646f63746f722e7661642e7265706169722e72657374617274 = restart the journal, then rerun { "solstone journal doctor" }; if it still times out, reinstall the journal

m6a6f75726e616c2e636c692e646f63746f722e7661642e7265706169722e726573746172742e617070 = restart the journal app, then rerun { "solstone journal doctor" }; if it still times out, reinstall the journal app

m6a6f75726e616c2e636c692e646f63746f722e7661642e737061776e = { "VAD" } helper could not be executed: { $cause }

m6a6f75726e616c2e636c692e646f63746f722e7661642e74696d656f7574 = { "VAD" } helper did not exit within { $timeout }

m6a6f75726e616c2e636c692e646f63746f722e7661642e756e7265736f6c766564 = could not resolve the { "VAD" } helper path: { $cause }

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e6f6e6e782e68656c706572 = { "ONNX" } helper is not the declared member of the signed app payload

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e6f6e6e782e6d656d626572 = signed { "ONNX" } app payload refuses { $member }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e6f6e6e782e706c6174666f726d = { "Windows" } { "ONNX" } package verification requires a { "Windows" } runtime

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e6f6e6e782e766572696669636174696f6e = could not verify the signed { "ONNX" } app payload: { $error }

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e706172616b6565742e65786563757461626c65 = could not determine the running journal executable: { $error }

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e706172616b6565742e6c61796f7574 = running journal executable is not in the package bin directory: { $path }

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e706172616b6565742e6d656d626572 = signed { "Parakeet" } app payload refuses { $member }: { $error }

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e706172616b6565742e706172656e74 = running journal executable has no containing directory: { $path }

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e706172616b6565742e706c6174666f726d = Windows { "Parakeet" } package verification requires a Windows runtime

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e706172616b6565742e726f6f74 = package bin directory has no package root: { $path }

m6a6f75726e616c2e636c692e646f63746f722e77696e646f77732e706172616b6565742e766572696669636174696f6e = could not verify the signed { "Parakeet" } app payload: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e617267756d656e74 = unexpected argument

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6461792e6578636c7573697665 = { "--day" } cannot be combined with { "--from" } or { "--to" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6461792e696e76616c6964 = { "DAY" } must be a real { "YYYYMMDD" } date

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6461792e7265717569726564 = { "--day" } requires { "YYYYMMDD" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e656e636f64652e66696e6973682e626f6479 = archive finish body: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e656e636f64652e66696e6973682e66696e616c697a65 = archive finish finalize: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e656e636f64652e6d656d626572 = archive write { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e656e636f64652e6d65746164617461 = invalid archive metadata { $field } { $value }: { $reason }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e656e636f64652e736f75726365 = archive source: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e656e636f64652e7772697465 = archive write: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e656e636f64652e777269746572 = invalid archive writer: { $reason }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6578706f72742e68656c70 = Usage: { "solstone journal archive export [--out PATH] [--quiet] [--day YYYYMMDD | --from YYYYMMDD [--to YYYYMMDD] | --to YYYYMMDD]" }{ "\u000a" }{ "\u000a" }create a { "ZIP" } of your journal, including settings and credentials.{ "\u000a" }processing logs, rebuildable indexes and caches, locks and temporary files are left out.{ "\u000a" }with a date filter, only the selected days are included.

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6578706f7274732e756e73616665 = unsafe exports directory: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e617263686976652e6f75747075742e636f6e7461696e732e612e6e756c6c2e62797465 = archive output contains a null byte

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e617263686976652e6f75747075742e656e64732e776974682e612e736570617261746f72 = archive output ends with a separator

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e617263686976652e6f75747075742e6861732e6e6f2e66696c652e6e616d65 = archive output has no file name

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e617263686976652e6f75747075742e6861732e6e6f2e706172656e74 = archive output has no parent

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e617263686976652e6f75747075742e6d7573742e62652e6f6e652e66696c652e6e616d65 = archive output must be one file name

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e617263686976652e6f75747075742e706172656e742e69732e6d697373696e672e6f722e756e73616665 = archive output parent is missing or unsafe

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e617263686976652e6f75747075742e706174682e69732e6e6f742e76616c69642e75746638 = archive output path is not valid { "UTF-8" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e617263686976652e6f75747075742e757365732e616e2e756e737570706f727465642e707265666978 = archive output uses an unsupported prefix

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e617263686976652e706172656e742e636f6d706f6e656e742e69732e696e76616c6964 = archive parent component is invalid

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e626c6f636b2e646576696365 = block device

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e6368617261637465722e646576696365 = character device

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e636f756c642e6e6f742e64657465726d696e652e7772697465722e706f736974696f6e = could not determine writer position

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e636f756c642e6e6f742e726561642e7772697465722e666c616773 = could not read writer flags

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e636f756c642e6e6f742e737461742e777269746572 = could not stat writer

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e6469726563746f7279 = directory

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e6669666f = fifo

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e66696c6573797374656d2e726f6f742e69732e6e6f742e612e6469726563746f7279 = filesystem root is not a directory

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e6861732e6e6f2e706172656e742e6469726563746f7279 = has no parent directory

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e6d7573742e62652e612e726567756c61722e66696c65 = must be a regular file

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e6d7573742e62652e656d707479 = must be empty

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e6d7573742e62652e6f70656e65642e666f722e77726974696e67 = must be opened for writing

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e6d7573742e62652e706f736974696f6e65642e61742e7468652e7374617274 = must be positioned at the start

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e6d7573742e62652e76616c69642e75746638 = must be valid { "UTF-8" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e6d7573742e6e616d652e6f6e652e6e6f726d616c2e66696c652e6c656166 = must name one normal file leaf

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e6d7573742e6e6f742e62652e6f70656e65642e696e2e617070656e642e6d6f6465 = must not be opened in append mode

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e6d7573742e7265736f6c76652e746f2e616e2e6162736f6c7574652e70617468 = must resolve to an absolute path

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e6f74686572 = other

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e726567756c61722e66696c65 = regular file

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e72656c61746976652e617263686976652e6f75747075742e72657175697265732e616e2e6162736f6c7574652e637764 = relative archive output requires an absolute cwd

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e736f636b6574 = socket

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e73796d6c696e6b = symlink

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e666163742e757466382e6d656d6265722e706174682e657863656564732e7a69702e7531362e627974652e6c696d6974 = { "UTF-8" } member path exceeds { "ZIP" } { "u16" } byte limit

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e66726f6d2e696e76616c6964 = { "FROM" } must be a real { "YYYYMMDD" } date

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e66726f6d2e7265717569726564 = { "--from" } requires { "YYYYMMDD" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e616d626967756974696573 = kind mismatch at { "entities/ambiguities.jsonl" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e6170706561726564 = { $relative } appeared while the import was running

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e62757379 = archive merge lock is busy at { $path }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e636c65616e7570 = failed cleaning extraction { $path }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e636f6d70726573736564 = { $i18nCategory ->
    [one] archive { $path } is { $bytes } byte; maximum is { $maximum }
   *[other] archive { $path } is { $bytes } bytes; maximum is { $maximum }
}

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e636f6f7264696e617465 = invalid planned recording coordinate

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e6469726563746f72792e72656d61696e73 = directory remains

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e656e637279707465642e65787472616374696f6e = encrypted archive entry during extraction: { $entry }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e656e637279707465642e76616c69646174696f6e = encrypted archive entry during validation: { $entry }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e656e74697479 = entity merge failed for { $entity_id }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e657870616e646564 = { $i18nCategory ->
    [one] archive expands to { $bytes } byte; maximum is { $maximum }
   *[other] archive expands to { $bytes } bytes; maximum is { $maximum }
}

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e65787472616374 = failed extracting { $archive } into { $dir }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e6661636574 = facet merge failed for { $facet }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e68616e646c696e67 = while handling { $path }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e68696464656e2e61726368697665 = this archive's hidden days of body data can't be read, so this archive can't be imported

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e68696464656e2e64657461696c73 = { $detail }; { $warning }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e68696464656e2e746172676574 = this journal can't read which days of body data you hid, so this archive can't be imported

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e68696464656e2e7761726e696e67 = { $i18nCategory ->
    [one] 1 day of body data was hidden in the archive, so it's hidden in this journal too; you can unhide it on the body page.
   *[other] { NUMBER($days, useGrouping: 0) } days of body data were hidden in the archive, so they're hidden in this journal too; you can unhide them on the body page.
}

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e696e76616c6964 = invalid archive { $path }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e6b696e64 = kind mismatch at { $relative }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e6c6567616379 = a { ".tgz" } or { ".tar.gz" } archive isn't supported. if it's an old transfer archive, export that day again as a { ".zip" } from the journal it came from and merge that, or merge this file into a new, empty journal on an earlier { "2.x" } release, export that journal as a { ".zip" }, and merge that { ".zip" } into your journal

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e6c6f636b = archive merge lock failed at { $path }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e6c6f67 = decision log write failed at { $path }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e6d61676963 = unrecognized archive magic

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e6d61726b6572 = stream marker update failed after archive content was published: { $detail }; published content was not rolled back

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e6d61726b65722e646179 = { $day }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e6d65726765 = import merge failed at { $path }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e6d697373696e67 = archive not found: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e6e616d65 = non-{ "UTF-8" } or missing path name

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e6f776e6572 = wait for the holder or inspect/remove only verified-stale owner metadata at { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e706174682e656e636f64696e67 = { $path } is not { "UTF-8" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e706174682e6f757473696465 = { $path } is outside the journal

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e7075626c697368 = archive merge publish failed: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e72656469726563742e656e74697479 = this journal merged '{ $source_id }' into '{ $survivor }', and merges can't be undone, so this archive can't be imported

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e72656469726563742e6661636574 = this journal had a facet named '{ $facet }' that was merged or renamed into another, and that name leads there now, so this archive can't be imported

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e7365676d656e74 = segment merge failed at { $path }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e7365676d656e742e6170706561726564 = segment appeared before archive publication

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e7365676d656e742e6368616e676564 = published archive segment changed

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e7365676d656e742e73796d6c696e6b = symbolic link in segment

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e7370616365 = insufficient free space at { $path }: { $available } available, { $required } required

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e7374616765 = staging write failed at { $path }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e73746167652e6368616e676564 = archive staging parent changed

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e73746f70706564 = import stopped

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e73796d6c696e6b = symbolic link

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e74726176657273616c = absolute or traversal path

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e756e616d626967756f7573 = archive has no unambiguous journal root

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e756e646f = { $detail }; undo incomplete: { $failures }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e756e646f2e636f6f7264696e617465 = coordinate { $relative }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e756e646f2e726573746f7265 = restore { $relative }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e756e646f2e72657772697465 = rewrite identity map: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e756e646f2e756e6c696e6b = unlink { $relative }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e756e696d706c656d656e746564 = import-sources: unimplemented: { $module }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e756e736166652e65787472616374696f6e = unsafe archive entry during extraction: { $entry }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696d706f72742e756e736166652e76616c69646174696f6e = unsafe archive entry during validation: { $entry }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696e636f6d706c657465 = { $committed } committed, { $skipped } skipped, { $failed } failed

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e696f73 = unavailable on { "iOS" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6d657267652e68656c70 = Usage: { "solstone journal archive merge SOURCE [--dry-run] [--json]" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6d657461646174612e726561736f6e2e696e76616c69642e63616c656e6461722e646174652e6f722e74696d65 = invalid calendar date or time

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6d657461646174612e726561736f6e2e696e76616c69642e646179 = invalid day

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6d657461646174612e726561736f6e2e696e76616c69642e686f7572 = invalid hour

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6d657461646174612e726561736f6e2e696e76616c69642e6d696e757465 = invalid minute

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6d657461646174612e726561736f6e2e696e76616c69642e6d6f6e7468 = invalid month

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6d657461646174612e726561736f6e2e696e76616c69642e7365636f6e64 = invalid second

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6d657461646174612e726561736f6e2e6d7573742e636f6e7461696e2e6469676974732e696e2e6e756d657269632e706f736974696f6e73 = must contain digits in numeric positions

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6d657461646174612e726561736f6e2e6d7573742e6e6f742e62652e656d707479 = must not be empty

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6d657461646174612e726561736f6e2e6d7573742e6e6f742e636f6e7461696e2e61736369692e636f6e74726f6c2e63686172616374657273 = must not contain ASCII control characters

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6d657461646174612e726561736f6e2e6d7573742e7573652e797979792e6d6d2e64647468682e6d6d2e73737a = must use YYYY-MM-DDTHH:MM:SSZ

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6d657461646174612e726561736f6e2e7365636f6e642e6d7573742e62652e696e2e7365636f6e6473 = second must be in 0..=59

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6d657461646174612e726561736f6e2e796561722e6d7573742e62652e696e2e7965617273 = year must be in 1980..=2107

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6e656761746976652e6964656e74697479 = source identity is negative

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6e656761746976652e73697a65 = regular-file size is negative

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6f75742e7265717569726564 = { "--out" } requires { "PATH" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6f75747075742e706172656e74 = archive output has no parent

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e6f75747075742e74726565 = output is inside exported { $family } tree

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e70726576696577 = Archive merge (days: { $days }): 0 committed, { $skipped } skipped, 0 failed.

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7075626c6973682e636c65616e7570 = clean archive temporary file: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7075626c6973682e63726561746574656d70 = create archive temporary file: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7075626c6973682e7075626c697368 = publish archive: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7075626c6973682e73796e636469726563746f7279 = sync archive directory: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7075626c6973682e73796e6374656d70 = sync archive temporary file: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e72616e6765 = { "--from" } must not be after { "--to" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e726f6f742e6e616d65 = journal root has no file name

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e726f6f742e706172656e74 = journal root has no parent

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736372617463682e6e616d65 = could not allocate an exclusive archive scratch file

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736b6970706564 = { "solstone journal archive export" }: skipped top-level entries: { $skipped }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e627564676574 = unsupported journal root { $path }: archive source exceeds its supported inventory budget

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e6368616e676564 = journal source changed at { $member }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e6c6973742e6a6f75726e616c2e6469726563746f7279 = list journal directory: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e6c6973742e6a6f75726e616c2e6469726563746f72792e6d656d626572 = list journal directory { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e6d656d626572 = { $operation } { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e6f70656e2e6a6f75726e616c2e6469726563746f72792e666f722e6c697374696e67 = open journal directory for listing: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e6f70656e2e6a6f75726e616c2e6469726563746f72792e666f722e6c697374696e672e6d656d626572 = open journal directory for listing { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e6f70656e2e6a6f75726e616c2e6469726563746f72792e6d656d626572 = open journal directory { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e6f70656e2e6a6f75726e616c2e66696c65 = open journal file: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e6f70656e2e6a6f75726e616c2e66696c652e6d656d626572 = open journal file { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e6f70656e2e72657461696e65642e6a6f75726e616c2e726f6f74 = open retained journal root: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e6f70656e2e72657461696e65642e6a6f75726e616c2e726f6f742e6d656d626572 = open retained journal root { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e726561642e696e76656e746f726965642e66696c65 = read inventoried file: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e726561642e696e76656e746f726965642e66696c652e6d656d626572 = read inventoried file { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e726561642e726567756c61722e66696c652e73697a65 = read regular-file size: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e726561642e726567756c61722e66696c652e73697a652e6d656d626572 = read regular-file size { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e726561642e736f757263652e66696c652e6964656e746974792e6d656d626572 = read source file identity { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e737461742e696e76656e746f726965642e6469726563746f7279 = stat inventoried directory: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e737461742e696e76656e746f726965642e6469726563746f72792e6d656d626572 = stat inventoried directory { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e737461742e696e76656e746f726965642e66696c65 = stat inventoried file: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e737461742e696e76656e746f726965642e66696c652e6d656d626572 = stat inventoried file { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e737461742e6a6f75726e616c2e656e747279 = stat journal entry: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e737461742e6a6f75726e616c2e656e7472792e6d656d626572 = stat journal entry { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e737461742e6f70656e65642e696e76656e746f726965642e6469726563746f7279 = stat opened inventoried directory: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e737461742e6f70656e65642e696e76656e746f726965642e6469726563746f72792e6d656d626572 = stat opened inventoried directory { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e737461742e6f70656e65642e696e76656e746f726965642e66696c65 = stat opened inventoried file: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e737461742e6f70656e65642e696e76656e746f726965642e66696c652e6d656d626572 = stat opened inventoried file { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e737461742e6f70656e65642e6a6f75726e616c2e6469726563746f7279 = stat opened journal directory: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e737461742e6f70656e65642e6a6f75726e616c2e6469726563746f72792e6d656d626572 = stat opened journal directory { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e737461742e6f70656e65642e6a6f75726e616c2e66696c65 = stat opened journal file: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e696f2e737461742e6f70656e65642e6a6f75726e616c2e66696c652e6d656d626572 = stat opened journal file { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e6d697373696e67 = { "SOURCE" } does not exist: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e726567756c6172 = { "SOURCE" } must be a regular file

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e7265717569726564 = { "SOURCE" } is required

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e756e73616665 = unsafe journal entry { $member }: { $kind }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e61726d2e7265637572736976652e6e616d6573706163652e7769746e657373 = arm recursive namespace witness: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e61726d2e7265637572736976652e6e616d6573706163652e7769746e6573732e6d656d626572 = arm recursive namespace witness { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e626f72726f772e61646d69747465642e6a6f75726e616c2e726f6f74 = borrow admitted journal root: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e626f72726f772e61646d69747465642e6a6f75726e616c2e726f6f742e6d656d626572 = borrow admitted journal root { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e626f72726f772e61646d69747465642e6a6f75726e616c2e726f6f742e6e616d6573706163652e7761746368 = borrow admitted journal-root namespace watch: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e626f72726f772e61646d69747465642e6a6f75726e616c2e726f6f742e6e616d6573706163652e77617463682e6d656d626572 = borrow admitted journal-root namespace watch { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e63616e63656c2e7265637572736976652e6e616d6573706163652e7769746e657373 = cancel recursive namespace witness: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e63616e63656c2e7265637572736976652e6e616d6573706163652e7769746e6573732e6d656d626572 = cancel recursive namespace witness { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e636865636b2e7265637572736976652e6e616d6573706163652e7769746e657373 = check recursive namespace witness: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e636865636b2e7265637572736976652e6e616d6573706163652e7769746e6573732e6d656d626572 = check recursive namespace witness { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e6372656174652e7265637572736976652e6e616d6573706163652e7769746e6573732e6576656e74 = create recursive namespace witness event: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e6372656174652e7265637572736976652e6e616d6573706163652e7769746e6573732e6576656e742e6d656d626572 = create recursive namespace witness event { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e647261696e2e7265637572736976652e6e616d6573706163652e7769746e657373 = drain recursive namespace witness: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e647261696e2e7265637572736976652e6e616d6573706163652e7769746e6573732e6d656d626572 = drain recursive namespace witness { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e6c6973742e72657461696e65642e6469726563746f7279 = list retained directory: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e6c6973742e72657461696e65642e6469726563746f72792e6d656d626572 = list retained directory { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e6f70656e2e72657461696e65642e72656c61746976652e656e747279 = open retained relative entry: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e6f70656e2e72657461696e65642e72656c61746976652e656e7472792e6d656d626572 = open retained relative entry { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e70617273652e72657461696e65642e6469726563746f72792e6c697374696e67 = parse retained directory listing: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e70617273652e72657461696e65642e6469726563746f72792e6c697374696e672e6d656d626572 = parse retained directory listing { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e71756572792e72657461696e65642e656e7472792e61747472696275746573 = query retained entry attributes: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e71756572792e72657461696e65642e656e7472792e617474726962757465732e6d656d626572 = query retained entry attributes { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e71756572792e72657461696e65642e656e7472792e6964656e74697479 = query retained entry identity: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e71756572792e72657461696e65642e656e7472792e6964656e746974792e6d656d626572 = query retained entry identity { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e71756572792e72657461696e65642e656e7472792e6d65746164617461 = query retained entry metadata: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e71756572792e72657461696e65642e656e7472792e6d657461646174612e6d656d626572 = query retained entry metadata { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e71756572792e72657461696e65642e656e7472792e74797065 = query retained entry type: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e71756572792e72657461696e65642e656e7472792e747970652e6d656d626572 = query retained entry type { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e726561642e6f627365727665642e66696c65 = read observed file: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e726561642e6f627365727665642e66696c652e6d656d626572 = read observed file { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e72657665726966792e72657461696e65642e72656c61746976652e6469726563746f72792e6964656e74697479 = reverify retained relative directory identity: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e72657665726966792e72657461696e65642e72656c61746976652e6469726563746f72792e6964656e746974792e6d656d626572 = reverify retained relative directory identity { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e73697a652e636865636b65642e726561642e627566666572 = size checked-read buffer: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e73697a652e636865636b65642e726561642e6275666665722e6d656d626572 = size checked-read buffer { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e74726163652e72657461696e65642e72656c61746976652e6469726563746f72792e6c697374696e672e6f70656e = trace retained relative directory listing open: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e74726163652e72657461696e65642e72656c61746976652e6469726563746f72792e6c697374696e672e6f70656e2e6d656d626572 = trace retained relative directory listing open { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e756e737570706f727465642e61726d2e7265637572736976652e6e616d6573706163652e7769746e657373 = unsupported journal root { $path }: arm recursive namespace witness

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e756e737570706f727465642e63616e63656c2e7265637572736976652e6e616d6573706163652e7769746e657373 = unsupported journal root { $path }: cancel recursive namespace witness

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e756e737570706f727465642e636865636b2e7265637572736976652e6e616d6573706163652e7769746e657373 = unsupported journal root { $path }: check recursive namespace witness

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e756e737570706f727465642e647261696e2e7265637572736976652e6e616d6573706163652e7769746e657373 = unsupported journal root { $path }: drain recursive namespace witness

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e756e737570706f727465642e776169742e666f722e7265637572736976652e6e616d6573706163652e7769746e657373 = unsupported journal root { $path }: wait for recursive namespace witness

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e776169742e666f722e7265637572736976652e6e616d6573706163652e7769746e657373 = wait for recursive namespace witness: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e736f757263652e77696e646f77732e776169742e666f722e7265637572736976652e6e616d6573706163652e7769746e6573732e6d656d626572 = wait for recursive namespace witness { $member }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e73706163652e6f766572666c6f77 = available disk space overflow

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e73706163652e706c6174666f726d = disk-space inspection is unsupported on this platform

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e73756d6d617279 = Archive merge: { $committed } committed, { $skipped } skipped, { $failed } failed.

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7461726765742e6368616e676564 = archive output route changed during export: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7461726765742e636f6c6c6973696f6e = archive output already exists: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7461726765742e696e76616c6964 = invalid archive output { $path }: { $reason }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7461726765742e696f = { $operation } failed for { $path }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7461726765742e696f2e6f70656e2e617263686976652e6f75747075742e66696c6573797374656d2e726f6f74 = open archive output filesystem root failed for { $path }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7461726765742e696f2e6f70656e2e617263686976652e6f75747075742e706172656e74 = open archive output parent failed for { $path }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7461726765742e696f2e72657461696e2e61646d69747465642e617263686976652e6f75747075742e706172656e74 = retain admitted archive output parent failed for { $path }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7461726765742e696f2e726576616c69646174652e61646d69747465642e617263686976652e6f75747075742e706172656e74 = revalidate admitted archive output parent failed for { $path }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7461726765742e696f2e737461742e61646d69747465642e617263686976652e6f75747075742e706172656e74 = stat admitted archive output parent failed for { $path }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7461726765742e696f2e737461742e617263686976652e6f75747075742e66696c6573797374656d2e726f6f74 = stat archive output filesystem root failed for { $path }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7461726765742e696f2e737461742e617263686976652e6f75747075742e66696e616c2e6e616d65 = stat archive output final name failed for { $path }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7461726765742e696f2e737461742e617263686976652e6f75747075742e706172656e74 = stat archive output parent failed for { $path }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7461726765742e696f2e737461742e6f70656e65642e617263686976652e6f75747075742e706172656e74 = stat opened archive output parent failed for { $path }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7461726765742e696f2e737461742e72657461696e65642e617263686976652e6f75747075742e706172656e74 = stat retained archive output parent failed for { $path }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7461726765742e756e73616665 = archive output is an unsafe { $kind }: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7461726765742e77696e646f77732e6368616e676564 = archive output changed: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7461726765742e77696e646f77732e696f = archive output { $path }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7461726765742e77696e646f77732e756e73616665 = unsafe archive output { $path }: { $reason }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e74656d702e6e616d65 = could not allocate a unique archive temporary name

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e746f2e696e76616c6964 = { "TO" } must be a real { "YYYYMMDD" } date

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e746f2e7265717569726564 = { "--to" } requires { "YYYYMMDD" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e617263686976652e7a65726f2e7772697465 = archive writer returned zero bytes for a nonempty write

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e617267756d656e742e696f73 = unavailable on { "iOS" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e617267756d656e742e756e6578706563746564 = unexpected argument

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e636f6e74656e742e756e73616665 = unsafe facet content: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e61646f707465642e68656164696e67 = Adopted orphan facets

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e61646f7074696f6e2e6661696c6564 = { $destination } (adoption failed: { $error })

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e61646f7074696f6e2e756e6d6572676564 = { $i18nCategory ->
    [one] { $destination } (adoption failed: { $error }; { $unmerged } was not merged)
   *[other] { $destination } (adoption failed: { $error }; { $unmerged } were not merged)
}

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e617267756d656e742e6d65726765 = { "--merge" } requires { "--adopt" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e636f6c6c6973696f6e2e68656164696e67 = Regular-file collisions

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e656d707479 = Nothing to report.

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6661696c65642e68656164696e67 = Failed orphan facets

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6669782e696f73 = { "--fix" } is unavailable on { "iOS" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e666f6c646572732e68696464656e = Folders whose names start with a dot are never facets, so these facet declarations are ignored: { $folders }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e67726f75702e616d626967756f7573 = { $members }: named like { $facets }; rename the folder to match one, then run with { "--adopt" } { "--merge" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e67726f75702e64616d61676564 = { $members }: named like { $facets }, whose { "facet.json" } needs repair first

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e68656c70 = Usage: { "solstone journal facet doctor [--fix] [--adopt [--merge]]" }{ "\u000a" }{ "       solstone journal facet doctor --retire NAME (--into FACET | --deleted)" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e686973746f72792e6661696c6564 = action log scan failed: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e686973746f72792e6d6572676564 = { $name } was merged into { $destination }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e686973746f72792e7265636f72646564 = Recorded retired facet names from the action logs:

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e686973746f72792e72656e616d6564 = { $name } was renamed into { $destination }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e686973746f72792e726575736564 = These facets were renamed or merged away earlier and their names later given to a new facet, so older material that names them can't be told apart. Nothing was changed: { $names }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e686973746f72792e756e6b6e6f776e = The action logs mention these names but not where they went: { $names }. If material still names one, record it with { "--retire NAME --into FACET" } or { "--deleted" }.

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e686973746f72792e756e7265636f72646564 = Facet names the action logs show were retired but aren't recorded (run with { "--fix" } to record them):

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6c6566746f766572732e6661696c6564 = merge leftovers couldn't be read: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6c6566746f766572732e6b657074 = Leftover folders from a facet merge that may hold the only copy of some files. Don't delete them; run with { "--fix" } to move them to the trash: { $folders }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6c6566746f766572732e6d6f766564 = Moved leftover folders from a facet merge to the trash, as they were: { $folders }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6c6566746f766572732e73616665 = Leftover folders from an interrupted facet merge, safe to delete once no merge is running: { $folders }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6c696e6b2e6475706c69636174652e6f70656e = { $facet }: '{ $id }' is linked from more than one folder ({ $folders })

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6c696e6b2e6475706c69636174652e7265706169726564 = { $facet }: '{ $id }' was linked from more than one folder ({ $folders })

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6c696e6b2e6d65726765642e6f70656e = { $i18nCategory ->
    [one] { $facet }: '{ $id }' was merged into '{ $successor }', and folder { $folders } still links it
   *[other] { $facet }: '{ $id }' was merged into '{ $successor }', and folders { $folders } still link it
}

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6c696e6b2e6d65726765642e7265706169726564 = { $i18nCategory ->
    [one] { $facet }: '{ $id }' was merged into '{ $successor }', and folder { $folders } linked it
   *[other] { $facet }: '{ $id }' was merged into '{ $successor }', and folders { $folders } linked it
}

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6c696e6b2e6d69736e616d65642e6f70656e = { $facet }: '{ $id }' is linked from folder { $folders }, not a folder named '{ $id }'

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6c696e6b2e6d69736e616d65642e7265706169726564 = { $facet }: '{ $id }' was linked from folder { $folders }, not a folder named '{ $id }'

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6c696e6b2e756e7265616461626c65 = { $facet }: folder { $folders } has a link that can't be read or used

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6c696e6b2e756e7265616461626c652e636865636b = { $facet }: folder { $folders } has a link that can't be read or used; { "--fix" } leaves it as it is

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6c696e6b2e756e7265616461626c652e756e64656369646564 = { $facet }: folder { $folders } has a link that can't be read or used ({ "--fix" } can't tell which entity it links, so it leaves it)

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6c696e6b732e636865636b = Entity links to repair (run with { "--fix" } to repair the ones that can be):

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6c696e6b732e636865636b2e6661696c6564 = entity link check failed: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6c696e6b732e6c656674 = Entity links left as they are:

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6c696e6b732e7265706169722e6661696c6564 = entity link repair failed: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6c696e6b732e7265706169726564 = Entity links repaired:

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6c696e6b732e72657363616e = Run '{ "solstone journal indexer --rescan-full" }' to refresh the index.

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6c6f636b2e6661696c6564 = the facet lock wasn't taken: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6d61696e74656e616e63652e68656164696e67 = Committed merge maintenance failures

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6d657267652e636f6c6c6973696f6e = { $source } -> { $destination }: { $path } (kept { $retained })

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6d657267652e6661696c6564 = { $source } -> { $destination } (merge failed before commit: { $error })

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6d657267652e696f73 = facet merging unavailable on { "iOS" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6d65726765642e68656164696e67 = Merged orphan facets

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6d65726765642e6e6f6e65 = No orphan facet was registered or merged.

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6d65726765732e696e636f6d706c657465 = one or more orphan facet merges committed with maintenance failures

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6f727068616e732e636f756e74 = { $i18nCategory ->
    [one] { NUMBER($count, useGrouping: 0) } orphan facet found. Run with { "--adopt" } to register it.
   *[other] { NUMBER($count, useGrouping: 0) } orphan facet(s) found. Run with { "--adopt" } to register them.
}

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6f727068616e732e68656164696e67 = Orphan facets:

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6f727068616e732e68656c64 = The action logs show these facet names were retired, so their folders are not registered; run with { "--fix" } to record them: { $folders }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6f727068616e732e6e6f6e65 = No orphan facets found.

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e6f727068616e732e72657469726564 = These folders have content under facet names that were merged or renamed into another facet, so they are not registered as facets; their files are left in place: { $folders }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e71756575652e6661696c6564 = the repair finished, but its search update could not be queued: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e7265666572656e6365 = - { $name }: { $count } ({ $first } to { $last })

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e7265666572656e6365732e636f6d6d616e64 = To make one reachable again, run: { "solstone journal facet doctor --retire NAME --into FACET" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e7265666572656e6365732e6661696c6564 = reference scan failed: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e7265666572656e6365732e6d697373696e67 = Segments name these facets, which don't exist and aren't recorded as retired. Agents limited to chosen facets can't see that material; everything else can:

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e726567697374657265642e6e6f6e65 = No orphan facet was registered.

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e726567697374726174696f6e2e6661696c6564 = registered { $slug }, but its record could not be written: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e7265706169722e6661696c6564 = retired facet names could not be repaired: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e72657061697265642e636f756e74 = { $i18nCategory ->
    [one] { NUMBER($repaired, useGrouping: 0) } orphan facet repaired. search is catching up.
   *[other] { NUMBER($repaired, useGrouping: 0) } orphan facet(s) repaired. search is catching up.
}

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e72657061697265642e68656164696e67 = Repaired orphan facets:

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e726570616972732e6661696c6564 = one or more orphan facet repairs failed

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e726570616972732e6661696c65642e636f756e74 = { $i18nCategory ->
    [one] { $i18nCategory-failed_orphans ->
        [one] { NUMBER($repaired, useGrouping: 0) } orphan facet repaired; { NUMBER($failed_orphans, useGrouping: 0) } orphan facet failed. Run '{ "solstone journal indexer --rescan-full" }' to refresh the index.
       *[other] { NUMBER($repaired, useGrouping: 0) } orphan facet repaired; { NUMBER($failed_orphans, useGrouping: 0) } orphan facet(s) failed. Run '{ "solstone journal indexer --rescan-full" }' to refresh the index.
    }
   *[other] { $i18nCategory-failed_orphans ->
        [one] { NUMBER($repaired, useGrouping: 0) } orphan facet(s) repaired; { NUMBER($failed_orphans, useGrouping: 0) } orphan facet failed. Run '{ "solstone journal indexer --rescan-full" }' to refresh the index.
       *[other] { NUMBER($repaired, useGrouping: 0) } orphan facet(s) repaired; { NUMBER($failed_orphans, useGrouping: 0) } orphan facet(s) failed. Run '{ "solstone journal indexer --rescan-full" }' to refresh the index.
    }
}

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e726570616972732e6d61696e74656e616e63652e636f756e74 = { $i18nCategory ->
    [one] { $i18nCategory-merges ->
        [one] { NUMBER($repaired, useGrouping: 0) } orphan facet repaired; { NUMBER($merges, useGrouping: 0) } merge committed but reported a maintenance failure after commit. See 'Committed merge maintenance failures' above. Run '{ "solstone journal indexer --rescan-full" }' to refresh the index.
       *[other] { NUMBER($repaired, useGrouping: 0) } orphan facet repaired; { NUMBER($merges, useGrouping: 0) } merge(s) committed but reported a maintenance failure after commit. See 'Committed merge maintenance failures' above. Run '{ "solstone journal indexer --rescan-full" }' to refresh the index.
    }
   *[other] { $i18nCategory-merges ->
        [one] { NUMBER($repaired, useGrouping: 0) } orphan facet(s) repaired; { NUMBER($merges, useGrouping: 0) } merge committed but reported a maintenance failure after commit. See 'Committed merge maintenance failures' above. Run '{ "solstone journal indexer --rescan-full" }' to refresh the index.
       *[other] { NUMBER($repaired, useGrouping: 0) } orphan facet(s) repaired; { NUMBER($merges, useGrouping: 0) } merge(s) committed but reported a maintenance failure after commit. See 'Committed merge maintenance failures' above. Run '{ "solstone journal indexer --rescan-full" }' to refresh the index.
    }
}

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e726574697265642e6d616c666f726d6564 = { "facets/retired.json" } does not parse ({ $detail }). Run with { "--fix" } to rebuild it; no name it reserves is freed.

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e726574697265642e72656275696c74 = { "facets/retired.json" } did not parse ({ $detail }); it was moved to { $path } and rebuilt.

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e726574697265642e7265736572766564 = These names could not be traced and are recorded as deleted: { $names }.

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e726574697265642e756e7265616461626c65 = { "facets/retired.json" } could not be read ({ $detail }). Check its permissions; it was left as it is.

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e7365617263682e6661696c6564 = search update could not be queued: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e7365617263682e717565756564 = search is catching up with the facet changes.

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e7365617263682e736b6970706564 = Search was not updated, because { "facets/retired.json" } can't be used yet.

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e7477696e732e68656164696e67 = Folders named like a facet you have:

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e7477696e732e6d65726765 = These are never registered beside it. Run with { "--adopt" } { "--merge" } to fold them into it; a merge is permanent, and a file both have keeps the facet's copy.

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e7477696e732e70726576696577 = To see what one would bring first: { "solstone journal facet merge" } { $source } { "--into" } { $destination } { "--dry-run" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e756e636c6561722e68656164696e67 = Folders left as they are

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e76617269616e74732e68656164696e67 = Name-variant groups:

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e76617269616e74732e6d65726765 = These were not registered. Run with { "--adopt" } { "--merge" } to collapse them into one.

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e646f63746f722e76617269616e74732e7265676973746572 = Run with { "--adopt" } { "--merge" } to collapse name variants before registering them.

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e656e7472792e756e73616665 = unsafe facet entry: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e68696464656e2e64616d61676564 = the hidden names at { $path } can't be read, so nothing was changed: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e68696464656e2e656d707479 = no name was given

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e68696464656e2e6669656c642e74657874 = { $key } is not text

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e68696464656e2e6669656c642e756e6b6e6f776e = unknown field { $key }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e68696464656e2e6e616d65732e6c697374 = { "names" } is not a list

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e68696464656e2e726f772e656d707479 = name is empty

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e68696464656e2e726f772e6572726f72 = name { $index }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e68696464656e2e726f772e6f626a656374 = not an object

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6a736f6e2e6f626a656374 = { $path } is not a { "JSON" } object

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6c696e6b2e636f6e666c696374 = { $path } has different contents in the folders being combined, so nothing was combined

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6c696e6b2e66696c652e656e636f64696e67 = { $path } is not { "UTF-8" } text

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6c696e6b2e6964732e657868617573746564 = notes have run out of id numbers

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6c696e6b2e696e76616c69642e6e616d65 = '{ $name }' can't be used as an entity id

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6c696e6b2e6e6f742e66696c65 = { $path } is a folder or link, not a file, so nothing was combined

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6c696e6b2e6f62736572766174696f6e2e636f7272757074 = corrupt observation in { $source }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6c696e6b2e6f62736572766174696f6e2e6d616c666f726d6564 = malformed observation in { $source } at line { $line }: { $reason }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6c696e6b2e6f63637570696564 = folder '{ $id }' holds another entity or can't be read

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6c696e6b2e7265706169722e64657374696e6174696f6e = the folder for '{ $id }' in the facet being merged into holds another entity or can't be read, so nothing was changed; run '{ "solstone journal facet doctor" }' to see what needs repair

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6c696e6b2e7265706169722e6e616d6564 = the folder for '{ $id }' in the '{ $facet }' facet holds another entity or can't be read, so nothing was changed; run '{ "solstone journal facet doctor" }' to see what needs repair

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6c696e6b2e737563636573736f722e6d697373696e67 = the entity it joined can't be found

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6c6f672e616374696f6e2e7175657565 = action log search update could not be queued: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6c6f672e646574656374696f6e732e7175657565 = facet detections not queued for search facet={ $facet } cause={ $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6c6f672e74726173682e756e66696e6973686564 = trash item { $id }: moved, finishing later: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e617267756d656e742e636f6e6669726d = a merge is permanent and can't be undone; review it with { "--dry-run" }, then run it again with { "--yes" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e617267756d656e742e64657374 = { "--into" } requires { "DEST" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e617267756d656e742e696e746f = { "--into DEST" } is required

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e617267756d656e742e696e76616c6964 = invalid { "SOURCE" } or { "DEST" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e617267756d656e742e736f75726365 = { "SOURCE" } is required

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e636c65616e75702e6661696c6564 = the merge finished, but its leftover folders couldn't be removed: { $error }; run '{ "solstone journal facet doctor --fix" }' to tidy them

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e636f6c6c6973696f6e732e6b657074 = Both facets had these files. '{ $destination }' kept its own copy; the copy in '{ $source }' was kept, and '{ "solstone journal facet doctor --fix" }' moves it to the trash:

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e636f6c6c6973696f6e732e74726173686564 = Both facets had these files. '{ $destination }' kept its own copy; the copy in '{ $source }' is in the trash:

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e636f6d706c657465 = Merged '{ $source }' into '{ $destination }'. Index rebuild completed.

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e64657374696e6174696f6e2e616273656e74 = '{ $destination }' isn't a facet

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e64657374696e6174696f6e2e6964 = '{ $destination }' has no stable id yet; run '{ "solstone journal backfill-facet-ids" }' first

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e64657374696e6174696f6e2e72657469726564 = '{ $destination }' is recorded as a retired facet name; run '{ "solstone journal facet doctor" }' for details

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e646574656374696f6e732e6661696c6564 = its detections were not queued

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e6661696c6564 = facet merge failed

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e68656c70 = Usage: { "solstone journal facet merge SOURCE --into DEST (--dry-run | --yes) [--consent]" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e707265766965772e656d707479 = Nothing.

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e707265766965772e656e746974696573 = { $i18nCategory ->
    [one] { NUMBER($count, useGrouping: 0) } entity is linked in both facets; it would end up with one link, keeping every note.
   *[other] { NUMBER($count, useGrouping: 0) } entities are linked in both facets; each would end up with one link, keeping every note.
}

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e707265766965772e6669656c6473 = Entity fields where '{ $destination }' would keep its own, different value:

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e707265766965772e66696c6573 = Files both facets have that can't be combined. '{ $destination }' would keep its own copy, and the copy in '{ $source }' would go to the trash:

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e707265766965772e68656164696e67 = Dry run, nothing was changed. Merging '{ $source }' into '{ $destination }' would lose:

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e707265766965772e6e6f746573 = { $i18nCategory ->
    [one] { NUMBER($count, useGrouping: 0) } note would get a new number, after the notes already there.
   *[other] { NUMBER($count, useGrouping: 0) } notes would get a new number, after the notes already there.
}

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e707265766965772e7265636f726473 = Records that would be dropped because a different record with the same id is kept (the first record with each id is kept, reading '{ $destination }' first):

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e707265766965772e73657474696e6773 = The settings in the { "facet.json" } of '{ $source }', which would not be carried over.

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e71756575652e6661696c6564 = the repair finished, but its search update could not be queued: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e736372617463682e6661696c6564 = dry run could not remove its scratch copy { $path }: { $error }; nothing else was changed, and that folder can be deleted

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e736f757263652e616273656e74 = '{ $source }' isn't a facet

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e736f757263652e6465636c61726564 = '{ $source }' is a declared facet, not an orphan folder

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e736f757263652e6f6e6c79 = '{ $source }' is the only facet that isn't muted, and a journal keeps at least one; unmute '{ $destination }' first

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e736f757263652e72657469726564 = '{ $source }' is a retired facet name

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6d657267652e74726173682e6661696c6564 = the merge finished, but the files it replaced couldn't be moved to the trash: { $error }; they're kept, and '{ "solstone journal facet doctor --fix" }' moves them there

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6e616d652e656e636f64696e67 = facet name is not { "UTF-8" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6e616d652e756e73616665 = unsafe facet declaration for { $slug }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6e6f746966792e7175657565 = the search update could not be queued: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e6e6f746966792e736561726368 = search was not updated: { $reason }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e706174682e656e636f64696e67 = { $path } isn't { "UTF-8" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e706174682e706c61696e = { $path } isn't a plain path

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e6465636c61726174696f6e2e64616d61676564 = facet declaration is damaged: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e6465636c61726174696f6e2e657869737473 = facet declaration already exists: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e6465636c61726174696f6e2e696e76616c6964 = facet declaration is not an object: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e6465636c61726174696f6e2e6d697373696e67 = facet declaration is missing: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e6465636c61726174696f6e2e756e7265616461626c65 = facet declaration could not be read: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e69642e6475706c6963617465 = duplicate well-formed facet identifier already on disk: { $id }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e69642e656e74726f7079 = could not obtain random bytes for identifier: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e69642e657868617573746564 = exhausted retries generating a unique facet identifier

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e6c696e6b2e696e76616c6964 = facet entity link is not an object: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e6d616c666f726d65642e6163746976697479 = cannot read activity definitions at { $path } line { $line }: { $reason }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e6d616c666f726d65642e6f62736572766174696f6e = malformed observation { $source } line { $line }: { $reason }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e6d61726b65722e696e76616c6964 = facet entity-link repair completion marker is empty or malformed: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e6d696e696d756d = facet '{ $facet }' is the only enabled facet; a journal keeps at least one

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e6d697373696e672e6e616d6573 = missing { "names" } object

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e6e616d652e72657469726564 = the facet name '{ $name }' belonged to a facet that was merged or renamed, and leads to it now

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e6e6f742e6f626a656374 = not a { "JSON" } object

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e726561736f6e2e636f6e74656e74 = expected an object with nonblank string content

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e726561736f6e2e686973746f7279 = invalid { "history" } field

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e726561736f6e2e6964 = { "id" } must be an unsigned integer

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e726561736f6e2e6a736f6e = invalid { "JSON" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e726561736f6e2e6f626a656374 = expected an object

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e726561736f6e2e72656c6174696f6e = expected an object with nonblank string content or relation

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e726561736f6e2e72657469726564 = invalid { "retired" } field

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e726574697265642e636f6e666c696374 = the facet name '{ $name }' is already recorded as retired with a different outcome

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e726574697265642e64616d61676564 = { "facets/retired.json" } could not be read ({ $detail }); run '{ "solstone journal facet doctor --fix" }'

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265636f72642e736f757263652e736e617073686f74 = captured snapshot

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265746972652e617267756d656e74 = { "--retire" } needs { "NAME" } and then { "--into FACET" } or { "--deleted" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265746972652e64656c65746564 = Recorded '{ $name }' as deleted. search is catching up.

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265746972652e656e636f64696e67 = names must be { "UTF-8" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265746972652e666f6c646572 = '{ $name }' is a folder in this journal; only a name that no longer exists can be retired

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265746972652e6d6572676564 = Recorded '{ $name }' as merged into '{ $target }'. search is catching up.

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265746972652e6e616d65 = '{ $name }' isn't a facet name

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7265746972652e746172676574 = '{ $target }' isn't a facet with a stable id; run '{ "solstone journal backfill-facet-ids" }' first

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7472616e73616374696f6e2e657869737473 = transaction path already exists: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7472616e73616374696f6e2e726f6c6c6261636b = transaction failed and was rolled back: { $primary }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e7472616e73616374696f6e2e726f6c6c6261636b2e6661696c6564 = transaction failed ({ $primary }); rollback also failed ({ $rollback_error })

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e74726173682e616273656e74 = that is no longer in the trash

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e74726173682e62757379 = that is already being recovered or erased

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e74726173682e696f = your trash couldn't be changed: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e74726173682e6d697373696e67 = there is no such { $item } in your journal

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e74726173682e756e66696e6973686564 = it's in the trash, which will finish putting it away

m6a6f75726e616c2e636c692e6a6f75726e616c2e66616365742e74726173682e756e6b6e6f776e = this build can't recover that

m6a6f75726e616c2e636c692e6a6f75726e616c2e68656c702e6f76657276696577 = { "solstone journal" } - the journal on this computer{ "\u000a" }{ "\u000a" }Usage: { "solstone journal <command> [args...]" }{ "\u000a" }{ "\u000a" }{ "'journal'" } has moved to '{ "solstone journal" }'; use that command for local journal operations.{ "\u000a" }{ "\u000a" }Local commands:{ "\u000a" }{ $commands }{ "\u000a" }{ "\u000a" }Process commands:{ "\u000a" }{ $processes }{ "\u000a" }{ "\u000a" }Journal-local commands:{ "\u000a" }{ $local }{ "\u000a" }{ "\u000a" }Options:{ "\u000a" }{ "  " }{ "-h, --help" }{ "    " }Show this help{ "\u000a" }{ "  -V, --version Show version" }{ "\u000a" }{ "  -v, --verbose Enable verbose mode" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e68656c702e70617468 = usage: { "solstone journal path [-h]" }{ "\u000a" }{ "\u000a" }Print the resolved journal path.

m6a6f75726e616c2e636c692e6a6f75726e616c2e68656c702e726f6f74 = usage: { "solstone journal root [-h]" }{ "\u000a" }{ "\u000a" }Print the project root.

m6a6f75726e616c2e636c692e6a6f75726e616c2e68656c702e73657475702e636f7079 = usage: { "solstone journal setup-copy [-h] [--json]" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e68656c702e737461747573 = usage: { "solstone journal status [-h]" }{ "\u000a" }{ "\u000a" }Print whether the journal exists and how many days it has.

m6a6f75726e616c2e636c692e6a6f75726e616c2e68656c702e7573616765 = Usage: { "solstone journal <command> [args...]" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e68656c7065722e696e7370656374 = { "native-helper-current-exe" }: could not inspect the native journal executable: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e68656c7065722e6d697373696e672e6d6163 = { "native-helper-missing" }: { $path }. Reinstall the journal app.

m6a6f75726e616c2e636c692e6a6f75726e616c2e68656c7065722e6d697373696e672e6f74686572 = { "native-helper-missing" }: { $path }. Reinstall { "solstone-journal" }.

m6a6f75726e616c2e636c692e6a6f75726e616c2e68656c7065722e6d6f64652e6d6163 = { "native-helper-not-executable" }: { $path }. Reinstall the journal app.

m6a6f75726e616c2e636c692e6a6f75726e616c2e68656c7065722e6d6f64652e6f74686572 = { "native-helper-not-executable" }: { $path }. Reinstall { "solstone-journal" }.

m6a6f75726e616c2e636c692e6a6f75726e616c2e686f73742e706174682e75746638 = native journal path resolution failed: resolved journal path is not valid UTF-8

m6a6f75726e616c2e636c692e6a6f75726e616c2e686f73742e7374617475732e616273656e74 = Journal: { $path }{ "\u000a" }Source: { $source }{ "\u000a" }Exists: no{ "\u000a" }Days: { $days }

m6a6f75726e616c2e636c692e6a6f75726e616c2e686f73742e7374617475732e6469726563746f7279 = native journal status failed: journal root is not a directory: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e686f73742e7374617475732e657869737473 = Journal: { $path }{ "\u000a" }Source: { $source }{ "\u000a" }Exists: yes{ "\u000a" }Days: { $days }

m6a6f75726e616c2e636c692e6a6f75726e616c2e686f73742e7374617475732e696e7370656374 = native journal status failed: could not inspect { $path }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e686f73742e7374617475732e75746638 = native journal status failed: resolved journal path is not valid UTF-8

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e617267756d656e742e617267756d656e74 = unexpected argument

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e617267756d656e742e627564676574 = { "--budget-seconds" } needs a positive number

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e617267756d656e742e647261696e = { "--drain" } requires { "--apply" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e617267756d656e742e6475706c6963617465 = unexpected or duplicate argument

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e617267756d656e742e66696c65 = { "--rescan-file" } requires { "PATH" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e617267756d656e742e696f73 = unavailable on { "iOS" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e617267756d656e742e6d697373696e672e6167656e74 = { "--agent" } requires { "AGENT" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e617267756d656e742e6d697373696e672e646179 = { "--day" } requires { "DAY" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e617267756d656e742e6d697373696e672e6461792e66726f6d = { "--day-from" } requires { "DAY" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e617267756d656e742e6d697373696e672e6461792e746f = { "--day-to" } requires { "DAY" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e617267756d656e742e6d697373696e672e6661636574 = { "--facet" } requires { "FACET" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e617267756d656e742e6d697373696e672e6c696d6974 = { "--limit" } requires a non-negative integer

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e617267756d656e742e6d697373696e672e6f6666736574 = { "--offset" } requires a non-negative integer

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e617267756d656e742e6d697373696e672e73747265616d = { "--stream" } requires { "STREAM" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e617267756d656e742e6d697373696e672e746f70 = { "--top" } requires a non-negative integer

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e617267756d656e742e72657363616e2e636f6e666c696374 = { "--rescan-file" } cannot be combined with { "--rescan" } or { "--rescan-full" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e617267756d656e742e756e737570706f72746564 = unsupported file

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6275696c642e6275696c64696e67 = build: building

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6275696c642e636f6d706c657465 = build: complete

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6275696c642e756e6b6e6f776e = build: unknown

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e6174 = line { $line }: { $reason }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e696d706f7274 = calendar notes in { $rel } skipped: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e6c696e652e726570656174 = calendar note line { $line } repeats an event

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e6c696e652e736b6970 = calendar note line { $line } skipped: { $reason }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e7365676d656e74 = calendar notes skipped: { $rel } is not in a segment

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e73657474696e6773 = calendar event in { $path } is listed once the settings file can be read: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e736f75726365 = calendar notes are read only from calendar streams and calendar imports: { $rel }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e756e6c6973746564 = calendar note in { $path } not listed: { $reason }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e6163636f756e742e6d7573742e62652e612e737472696e672e6f722e6e756c6c = { "account" } must be a string or { "null" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e616c6c2e6461792e6d7573742e62652e747275652e6f722e66616c7365 = { "all_day" } must be { "true" } or { "false" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e617474656e6465652e7374617475732e6d7573742e62652e612e737472696e672e6f722e6e756c6c = attendee { "status" } must be a string or { "null" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e617474656e6465652e796f752e6d7573742e62652e747275652e6f722e66616c7365 = attendee { "you" } must be { "true" } or { "false" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e617474656e646565732e6d7573742e62652e616e2e6172726179 = { "attendees" } must be an array

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e6368616e67652e6d7573742e62652e61646465642e6368616e6765642e6f722e72656d6f766564 = { "change" } must be { "added, changed or removed" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e6368616e6765642e61742e66616c6c732e6f7574736964652e7468652e7365676d656e742e77696e646f77 = { "changed_at" } falls outside the segment window

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e6368616e6765642e61742e6d7573742e62652e612e74696d652e776974682e616e2e6f6666736574 = { "changed_at" } must be a time with an offset

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e656163682e617474656e6465652e6d7573742e62652e616e2e6f626a656374 = each attendee must be an object

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e656e642e63616e6e6f742e62652e726561642e61732e612e646174652e6f722e74696d65 = { "end" } cannot be read as a date or time

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e656e642e69732e6265666f72652e7374617274 = { "end" } is before start

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e656e642e6d7573742e62652e612e737472696e672e6f722e6e756c6c = { "end" } must be a string or { "null" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e6576656e742e616e642e6f6363757272656e63652e7265706561742e616e2e6561726c6965722e6c696e65 = { "event" } and occurrence repeat an earlier line

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e6576656e742e6d7573742e62652e612e6e6f6e2e656d7074792e737472696e67 = { "event" } must be a non-empty string

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e657870616e6465642e6d7573742e62652e747275652e7768656e2e70726573656e74 = { "expanded" } must be { "true" } when present

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e6669656c642e6d697373696e67 = { $key } is missing

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e6669656c642e74797065 = { $key } must be a string or { "null" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e66696c652e656e636f64696e67 = file is not { "UTF-8" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e66696c652e686f6c64732e6e6f2e6e6f746573 = file holds no notes

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e66696c652e6c696e6573 = file holds more than 5,000 notes

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e66696c652e73697a65 = file holds more than 8 { "MiB" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e66696c656e616d65 = file name must be { "calendar_" } followed by 64 lowercase hex digits and { ".jsonl" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e696d706f72742e6d7573742e62652e612e6e6f6e2e656d7074792e737472696e67 = { "import" } must be a non-empty string

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e696d706f727465642e61742e6d7573742e62652e612e74696d652e776974682e616e2e6f6666736574 = { "imported_at" } must be a time with an offset

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e6c696e652e69732e656d707479 = line is empty

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e6c696e652e69732e6e6f742e612e6a736f6e2e6f626a656374 = line is not a { "JSON" } object

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e6f6363757272656e63652e63616e6e6f742e62652e726561642e61732e612e646174652e6f722e74696d65 = { "occurrence" } cannot be read as a date or time

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e6f6363757272656e63652e6d7573742e62652e612e737472696e672e6f722e6e756c6c = { "occurrence" } must be a string or { "null" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e6f7267616e697a65722e6d7573742e62652e616e2e6f626a6563742e6f722e6e756c6c = { "organizer" } must be an object or { "null" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e706572736f6e2e6669656c64 = { $what } { $key } must be a string or { "null" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e7365676d656e742e67726964 = segment must name one 300-second window on the journal's grid

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e7365676d656e742e6d7573742e6e616d652e612e7370616e2e6f6e2e6974732e646179 = segment must name a span on its day

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e73746172742e616e642e656e642e6d7573742e62652e64617465732e65786163746c792e7768656e2e616c6c2e6461792e69732e74727565 = { "start" } and { "end" } must be dates exactly when { "all_day" } is { "true" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e73746172742e616e642e656e642e6d7573742e626f74682e63617272792e616e2e6f66667365742e6f722e626f74682e686176652e6e6f6e65 = { "start" } and { "end" } must both carry an offset or both have none

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e73746172742e63616e6e6f742e62652e726561642e61732e612e646174652e6f722e74696d65 = { "start" } cannot be read as a date or time

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e73746172742e69732e6d697373696e67 = { "start" } is missing

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e63616c656e6461722e76616c69646174696f6e2e76657273696f6e = { "v" } must be 1

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6368756e6b65722e6163746976697479 = unsupported desktop activity version

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6368756e6b65722e63616c656e646172 = calendar note skipped: { $reason }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6368756e6b65722e6c696e6573 = { $i18nCategory ->
    [one] Collapsed whitespace on { NUMBER($collapsed, useGrouping: 0) } overlong Markdown line during indexing
   *[other] Collapsed whitespace on { NUMBER($collapsed, useGrouping: 0) } overlong Markdown line(s) during indexing
}

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e636c617373696669636174696f6e2e616273656e74 = classifications: database absent

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e636c617373696669636174696f6e2e636f6d706c657465 = classifications: complete (processed { $processed }, unclassified { $unclassified })

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e636c617373696669636174696f6e2e696e636f6d706c657465 = classifications: incomplete sidecar at cursor '{ $cursor }' (run '{ $repair }')

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e636c617373696669636174696f6e2e6d69736d61746368 = classifications: coverage mismatch at cursor '{ $cursor }' (processed { $processed }, remaining { $remaining }, missing { $missing }, unclassified { $unclassified })

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e636c617373696669636174696f6e2e6d697373696e67 = { $i18nCategory ->
    [one] chunk classification coverage mismatch: { NUMBER($missing, useGrouping: 0) } missing row
   *[other] chunk classification coverage mismatch: { NUMBER($missing, useGrouping: 0) } missing rows
}

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e636c617373696669636174696f6e2e70726f6772657373 = classifications: in progress at cursor '{ $cursor }' (processed { $processed }, remaining { $remaining }, missing { $missing }, unclassified { $unclassified })

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e636c617373696669636174696f6e2e7374616c6c = chunk classification backfill stalled: { $err }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e636c617373696669636174696f6e2e7374616c6c6564 = classifications: stalled at '{ $path }' (processed { $processed }, remaining { $remaining }, missing { $missing }, unclassified { $unclassified })

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e636c617373696669636174696f6e2e756e7265616479 = classifications: unready at cursor '{ $cursor }' (run '{ "solstone-core indexer path-lookup --apply" }')

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e64617461626173652e616273656e74 = index: absent

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e64617461626173652e66696c6573 = files: { $discovered } on disk, { $indexed } indexed, { $current } current, { $stale } stale, { $missing } missing, { $failed } failed, { $orphaned } orphaned, { $retained } kept until a full rescan, { $ineligible_rows } no longer indexable but still in search, { $unreadable } unreadable, { $ineligible } not indexable

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e64617461626173652e70726573656e74 = index: present

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e64617461626173652e73716c697465 = not a { "SQLite" } database

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e64617461626173652e756e7265616461626c65 = index: unreadable ({ $reason })

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e646973636f766572792e6c697374696e67 = glob traversal failed: attempting to read `{ $path }` resulted in an error: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e646973636f766572792e7061747465726e = invalid glob pattern: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e646973636f766572792e707265666978 = discovered path { $path } is not under root { $root }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e646973636f766572792e72656c6174697665 = discovered path is not valid { "UTF-8" }: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e646973636f766572792e726f6f74 = glob root is not valid { "UTF-8" }: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e647261696e2e726573756c74 = { $i18nCategory ->
    [one] applied { NUMBER($processed, useGrouping: 0) } search update; { $failed } failed; { NUMBER($remaining, useGrouping: 0) } { $i18nCategory-remaining ->
        [one] remains
       *[other] remain
    } queued.
   *[other] applied { NUMBER($processed, useGrouping: 0) } search updates; { $failed } failed; { NUMBER($remaining, useGrouping: 0) } { $i18nCategory-remaining ->
        [one] remains
       *[other] remain
    } queued.
}

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e656467652e626c6f62 = edge row contains an unsupported blob value

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e656467652e646179 = Invalid edge day: { $day }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e656467652e647374 = edge row requires non-empty string { "dst" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e656467652e6576656e74 = event timestamp is outside { "i64" } range

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e656467652e696e7465676572 = edge integer field { $field } is outside { "i64" } range

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e656467652e6a736f6e = edge source { "JSON" } parse failed for { $path }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e656467652e6b696e64 = Unknown edge kind: { $kind }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e656467652e70617468 = invalid observations path: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e656467652e7061796c6f6164 = { $source } payload must be a { "JSON" } object, got { $value_type }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e656467652e72656672657368 = Skipping edge refresh for { $rel }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e656467652e72656c6174696f6e = observation relation is missing { "kind" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e656467652e7365676d656e742e646179 = Invalid segment day: { $day }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e656467652e7365676d656e742e6b6579 = Invalid segment key: { $segment }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e656467652e7365676d656e742e74696d65 = segment timestamp is outside { "i64" } range

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e656467652e737263 = edge row requires non-empty string { "src" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e656467652e737472696e67 = cannot reproduce { "Python str()" } for { $field } value of type { $value_type }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e656467652e746172676574 = observation relation { "target_entity_id" } must be a non-empty string

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e656467652e74797065 = edge kind for { $source } must be a string, got { $value_type }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e656467652e756e737570706f72746564 = edge field { $field } does not support { $value_type }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6572726f722e66696c65 = rescan-file failed: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6572726f722e72656275696c64 = edge rebuild failed: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6572726f722e7265736574 = reset failed: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6572726f722e7363616e = scan failed: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e66696c65732e61637469766974792e6d656d626572736869702e696e73657274 = activity membership insert failed for { $rel }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e66696c65732e61637469766974792e70726f76656e616e63652e696e73657274 = activity provenance insert failed for { $rel }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e66696c65732e63616c656e6461722e6c697374696e67 = calendar listing failed for { $rel }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e66696c65732e63616c656e6461722e6e6f74652e696e73657274 = calendar note insert failed for { $rel }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e66696c65732e6368756e6b2e67726f75702e6f66667365742e696e73657274 = chunk group offset insert failed for { $rel }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e66696c65732e6368756e6b2e696e73657274 = chunk insert failed for { $rel }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e66696c65732e6368756e6b2e736f757263652e6d617070696e67 = chunk source mapping failed for { $rel }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e66696c65732e636c617373696669636174696f6e2e696e73657274 = classification insert failed for { $rel }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e66696c65732e636f6e74656e742e72656164 = content read failed for { $rel }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e66696c65732e6661636574 = facet declaration scan failed for { $rel }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e66696c65732e68696464656e = { $rel } is left out of search for now: { $cause }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e66696c65732e6d656d6f72792e636861696e2e73657269616c697a6174696f6e = memory chain serialization failed for { $rel }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e66696c65732e6d656d6f72792e6d697373696e67 = validated memory original missing for { $rel }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e66696c65732e6d656d6f72792e6f726967696e2e73657269616c697a6174696f6e = memory origin serialization failed for { $rel }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e66696c65732e6d656d6f72792e6f726967696e616c2e696e73657274 = memory original insert failed for { $rel }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e66696c65732e6d656d6f72792e757466 = memory original is not { "UTF-8" } for { $rel }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e66696c65732e7061727469616c = { $rel } is partly left out of search for now: { $cause }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e66696c65732e73637265656e2e746578742e736f757263652e696e73657274 = screen text source insert failed for { $rel }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e67656e65726174696f6e2e6c6f636b6564 = generation: { $generation } (this version reads up to { $known }; writable: no)

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e67656e65726174696f6e2e7772697461626c65 = generation: { $generation } (this version reads up to { $known }; writable: yes)

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e68656c702e647261696e = Usage: { "solstone journal indexer drain [--force] [--budget-seconds N] [--json]" }{ "\u000a" }apply queued search updates. { "--force" } includes delayed retries.{ "\u000a" }{ "--budget-seconds" } stops the drain after N seconds. a whole-journal update does not start with under 300 seconds left and stays queued.

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e68656c702e6d61696e = Usage: { "solstone journal indexer [--reset] [--rebuild-edges] [--rescan | --rescan-full | --rescan-file PATH] [-q [QUERY]] [--day DAY] [--day-from DAY] [--day-to DAY] [--facet FACET] [--agent AGENT] [--stream STREAM] [--limit N] [--offset N] [--top N]" }{ "\u000a" }{ "       solstone journal indexer drain [--force] [--budget-seconds N] [--json]" }{ "\u000a" }{ "       solstone journal indexer status [--json]" }{ "\u000a" }{ "       solstone journal indexer path-lookup [--apply] [--json]" }{ "\u000a" }{ "       solstone journal indexer classifications [--json] [--apply] [--drain]" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e68656c702e70617468 = Usage: { "solstone journal indexer path-lookup [--apply] [--json]" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e68656c702e737461747573 = Usage: { "solstone journal indexer status [--json]" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e68696464656e2e6163746976697479 = can't tell whether this activity is hidden: { $rel } can't be read: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e68696464656e2e64616d61676564 = can't tell whether this activity is hidden: { $path } has a line that isn't a record

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e68696464656e2e64617973 = can't tell which days of body data are hidden: { $path } can't be read

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e68696464656e2e7472617368 = can't tell what the journal is making again: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e68696464656e2e7765656b = can't tell what is left out of this week page: { $path } can't be read

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e68696464656e2e7765656b73 = can't tell what is left out of week pages: { $path } can't be read

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6d657267652e636f6d6d6974 = index is correct and merely uncompacted: could not commit { "FTS5" } merge: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6d657267652e6661696c6564 = index is correct and merely uncompacted: { "FTS5" } merge failed: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6d657267652e7374617274 = index is correct and merely uncompacted: could not start { "FTS5" } merge transaction: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6d6967726174696f6e2e6368696c64 = segment aggregate migration retained { $rel_segment }: child file is not current: { $rel }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6d6967726174696f6e2e646973636f76657279 = segment aggregate migration retained { $rel_segment }: talent discovery failed: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6d6967726174696f6e2e66696c6573 = index files remain after migration: { $paths }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6d6967726174696f6e2e6d74696d65 = segment aggregate migration retained { $rel_segment }: mtime read failed for { $rel }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6d6967726174696f6e2e72657461696e6564 = segment aggregate migration retained { $rel }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6d6967726174696f6e2e756e636c6173736966696564 = segment aggregate migration retained { $rel_segment }: unclassified talent Markdown { $rel }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6f726967696e616c2e696e636f6d706c657465 = memory original is incomplete for { $rel }: { $state }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6f726967696e616c2e72657461696e6564 = memory original could not be checked for { $rel }; cached index retained: { $state }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6f726967696e616c2e73746174652e616273656e74 = absent

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6f726967696e616c2e73746174652e636f7272757074 = corrupt

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6f726967696e616c2e73746174652e64656c65746564 = deleted

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6f726967696e616c2e73746174652e7265616479 = ready

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6f726967696e616c2e73746174652e737461676564 = staged

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6f726967696e616c2e73746174652e756e617661696c61626c65 = unavailable ({ $detail })

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e6f726967696e616c2e73746174652e756e7265616479 = unready

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e706174682e7265616479 = path lookup: ready

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e706174682e756e7265616479 = path lookup: unready

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e70726f67726573732e616273656e74 = classification: absent

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e70726f67726573732e636f6d706c657465 = classification: complete

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e70726f67726573732e696e636f6d706c657465 = classification: incomplete

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e70726f67726573732e7374616c6c6564 = classification: stalled at { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e70726f67726573732e756e6b6e6f776e = classification: stalled at unknown path

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e71756572792e616273656e74 = journal index is absent: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e71756572792e636f6c756d6e2e6167656e74 = Agent

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e71756572792e636f6c756d6e2e646179 = Day

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e71756572792e636f6c756d6e2e6661636574 = Facet

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e71756572792e656d707479 = journal index is empty: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e71756572792e6572726f72 = { "indexer" }: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e71756572792e696e707574 = stdin read failed: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e71756572792e6c6f636b6564 = journal index is locked ({ $path }): { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e71756572792e6d6f7265 = ... +{ $remaining } more

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e71756572792e6e6f6e65 = No results found

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e71756572792e6f7574707574 = stdout write failed: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e71756572792e6f776e6572 = connection query needs the owner

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e71756572792e70726f6d7074 = search>

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e71756572792e73686f77696e67 = { $i18nCategory ->
    [one] Showing { $first }-{ $last } of { NUMBER($total, useGrouping: 0) } result
   *[other] Showing { $first }-{ $last } of { NUMBER($total, useGrouping: 0) } results
}

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e71756572792e746f74616c = { $i18nCategory ->
    [one] Total: { NUMBER($total, useGrouping: 0) } chunk
   *[other] Total: { NUMBER($total, useGrouping: 0) } chunks
}

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e71756572792e756e7265616461626c65 = journal index is unreadable ({ $path }): { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e726563656970742e616273656e74 = last scan: not recorded

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e726563656970742e66756c6c2e66696e6973686564 = last scan: full at { $time } ms, indexed { $indexed }, removed { $removed }, failed { $failed }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e726563656970742e66756c6c2e73746f70706564 = last scan: full at { $time } ms stopped: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e726563656970742e6c696768742e66696e6973686564 = last scan: light at { $time } ms, indexed { $indexed }, removed { $removed }, failed { $failed }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e726563656970742e6c696768742e73746f70706564 = last scan: light at { $time } ms stopped: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e72656c2e6162736f6c757465 = { "rel" } must be journal-{ "rel" }ative

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e72656c2e6261636b736c617368 = { "rel" } must use { "POSIX" } separators

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e72656c2e656d707479 = { "rel" } must be non-empty

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e72656c2e696e76616c6964636f6d706f6e656e74 = { "rel" } must not contain empty, { "'.'" }, or { "'..'" } components

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e72656c2e7472617368 = { "rel" } must not name the trash

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7363616e2e63616c656e6461722e696e7669746573 = calendar invites not linked yet: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7363616e2e636f6e6e656374696f6e73 = No new connections were added in this rescan. Run { "solstone journal indexer --rebuild-edges" } to bring any still waiting up to date now.

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7363616e2e636f6e6e656374696f6e732e6661696c6564 = connections not checked against entity changes: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7363616e2e6465666572726564 = a full search pass is already working through these changes, so this check left them to it. run { "solstone journal indexer status" } for details.

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7363616e2e656467652e736b6970 = Skipping edge extraction for { $rel }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7363616e2e6661696c6564 = { $i18nCategory ->
    [one] Indexed { NUMBER($indexed, useGrouping: 0) } file, removed { $removed }, skipped { $skipped }, failed { $failed }
   *[other] Indexed { NUMBER($indexed, useGrouping: 0) } file(s), removed { $removed }, skipped { $skipped }, failed { $failed }
}

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7363616e2e6661696c757265 = { $i18nCategory ->
    [one] { "solstone journal indexer" }: { NUMBER($failed, useGrouping: 0) } update couldn't be indexed; see the warnings above
   *[other] { "solstone journal indexer" }: { NUMBER($failed, useGrouping: 0) } update(s) couldn't be indexed; see the warnings above
}

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7363616e2e696e76616c69642e7365676d656e74 = Skipping edge extraction for { $rel }: invalid segment key { $segment }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7363616e2e6d74696d65 = mtime read failed for { $rel }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7363616e2e72657461696e6564 = { $i18nCategory ->
    [one] light scan retained { NUMBER($count, useGrouping: 0) } stale row for day { $day }: discovery produced no files for that day; rerun with { "--rescan-full" } to remove it
   *[other] light scan retained { NUMBER($count, useGrouping: 0) } stale row(s) for day { $day }: discovery produced no files for that day; rerun with { "--rescan-full" } to remove them
}

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7363616e2e73756d6d617279 = { $i18nCategory ->
    [one] Indexed { NUMBER($indexed, useGrouping: 0) } file, removed { $removed }, skipped { $skipped }
   *[other] Indexed { NUMBER($indexed, useGrouping: 0) } file(s), removed { $removed }, skipped { $skipped }
}

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e737065616b65722e6475706c6963617465 = speaker edge transcript duplicate { "sentence_id" } { $sentence } for { $composite_id }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e737065616b65722e6d697373696e67 = speaker edge transcript missing for { $composite_id }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e737065616b65722e756e7265736f6c766564 = speaker edge transcript unresolved for { $composite_id }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7374617475732e636865636b = last search check: at { $time } ms, indexed { $indexed }, removed { $removed }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7374617475732e636865636b2e616273656e74 = last search check: not recorded

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7374617475732e636865636b2e6f766572647565 = last search check: overdue, it has run since then without finishing

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7374617475732e6661696c757265 = failure: { $reason }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7374617475732e6865616c7468 = health: { $state } ({ $description })

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7374617475732e717565756564 = queued: { $queued }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7374617475732e72656d6f76616c73 = removals still waiting: { $removals }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7374617475732e776169742e64617973 = oldest queued: waiting { $days } d { $hours } h

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7374617475732e776169742e686f757273 = oldest queued: waiting { $hours } h { $minutes } min

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7374617475732e776169742e6d696e75746573 = oldest queued: waiting { $minutes } min

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7374617475732e776169742e7365636f6e6473 = oldest queued: waiting { $seconds } s

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e73746f72652e61646d697373696f6e = index writer admission failed for { $operation }: { $cause }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e73746f72652e62757379 = index writer busy for { $operation }: { $cause }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e73746f72652e656e636f64696e67 = path is not valid { "UTF-8" }: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e73746f72652e67656e65726174696f6e = the search index was written by a newer version of { "solstone" } (index generation { $found }; this version reads up to { $known }), so this version leaves it unchanged

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e73746f72652e6c6f6f6b75702e6361757365 = { "indexer path-lookup --apply" }: { $cause }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e73746f72652e6d697373696e67 = file not found: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e73746f72652e6f726967696e616c = memory original could not be checked at { $path }; cached index retained; retry when the source is readable: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e73746f72652e6f757473696465 = file is outside journal directory: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e73747265616d2e6a736f6e = stream marker { "JSON" } failed for { $path }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e73747265616d2e70617468 = stream marker path failed for { $rel_segment }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e73747265616d2e72656164 = stream marker read failed for { $path }: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e696e64657865722e7761726e696e67 = warning: { $warning }

m6a6f75726e616c2e636c692e6a6f75726e616c2e6c6f63616c2e6469726563746f7279 = not a real directory: { $path }

m6a6f75726e616c2e636c692e6a6f75726e616c2e6c6f63616c2e6661696c757265 = { "solstone journal" } { $token }: { $message }

m6a6f75726e616c2e636c692e6a6f75726e616c2e6c6f63616c2e7265736f6c7574696f6e2e636f6e666967 = journal resolution failed: config decode failed

m6a6f75726e616c2e636c692e6a6f75726e616c2e6c6f63616c2e7265736f6c7574696f6e2e686f6d65 = journal resolution failed: home unavailable

m6a6f75726e616c2e636c692e6a6f75726e616c2e6c6f63616c2e756e6b6e6f776e = unknown local operation

m6a6f75726e616c2e636c692e6a6f75726e616c2e6e6f746966792e6572726f722e6661636574 = { "solstone journal notify" }: error: { $option } is no longer supported. Put facet selection in { "--action" }; for example, { "--action" } { "/app/entities?facet=work" }.

m6a6f75726e616c2e636c692e6a6f75726e616c2e6e6f746966792e6572726f722e696e7465676572 = { "solstone journal notify" }: error: argument { "--auto-dismiss" }: invalid int value: '{ $value }'

m6a6f75726e616c2e636c692e6a6f75726e616c2e6e6f746966792e6572726f722e6d657373616765 = { "solstone journal notify" }: error: the following arguments are required: { "message" }

m6a6f75726e616c2e636c692e6a6f75726e616c2e6e6f746966792e6572726f722e756e6b6e6f776e = { "solstone journal notify" }: error: unrecognized arguments: { $token }

m6a6f75726e616c2e636c692e6a6f75726e616c2e6e6f746966792e6572726f722e76616c7565 = { "solstone journal notify" }: error: argument { $option }: expected one argument

m6a6f75726e616c2e636c692e6a6f75726e616c2e6e6f746966792e6661696c6564 = Failed to send notification (is { "callosum" } running?)

m6a6f75726e616c2e636c692e6a6f75726e616c2e6e6f746966792e68656c70 = usage: { "solstone journal notify [-h] [--title TITLE] [--icon ICON] [--event EVENT]" }{ "\u000a" }{ "                  [--action ACTION] [--app APP] [--badge BADGE]" }{ "\u000a" }{ "                  [--auto-dismiss AUTO_DISMISS] [--no-dismiss] [-v] [-d]" }{ "\u000a" }                  { "message [message ...]" }{ "\u000a" }{ "\u000a" }Send a notification via { "callosum" }{ "\u000a" }{ "\u000a" }positional arguments:{ "\u000a" }{ "  message               " }notification message text{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  " }{ "-h, --help" }{ "            " }show this help message and exit{ "\u000a" }{ "  " }{ "--title TITLE" }{ "         " }notification title{ "\u000a" }{ "  " }{ "--icon ICON" }{ "           " }{ "Lucide" } icon name (default: { "mailbox" }){ "\u000a" }{ "  " }{ "--event EVENT" }{ "         " }event name (default: { "show)" }{ "\u000a" }{ "  " }{ "--action ACTION" }{ "       " }URL path to open on click{ "\u000a" }{ "  " }{ "--app APP" }{ "             " }source app name{ "\u000a" }{ "  " }{ "--badge BADGE" }{ "         " }badge text or number{ "\u000a" }{ "  --auto-dismiss AUTO_DISMISS" }{ "\u000a" }                        auto-dismiss after N milliseconds{ "\u000a" }{ "  " }{ "--no-dismiss" }{ "          " }make notification non-dismissible{ "\u000a" }{ "  " }{ "-v, --verbose" }{ "         " }Enable verbose output{ "\u000a" }{ "  " }{ "-d, --debug" }{ "           " }Enable debug logging

m6a6f75726e616c2e636c692e6a6f75726e616c2e6e6f746966792e73656e74 = Notification sent

m6a6f75726e616c2e636c692e6a6f75726e616c2e6e6f746966792e75746638 = native journal notify failed: owner arguments are not valid UTF-8

m6a6f75726e616c2e636c692e6a6f75726e616c2e70726f636573732e61646d697373696f6e = journal process admission failed: { $detail }

m6a6f75726e616c2e636c692e6a6f75726e616c2e70726f636573732e626f6479 = native journal process launch failed: no native body for { $token }

m6a6f75726e616c2e636c692e6a6f75726e616c2e70726f636573732e6c61756e6368 = native journal process launch failed: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e7265736f6c7574696f6e2e636f6e666967 = native journal resolution failed: config decode failed

m6a6f75726e616c2e636c692e6a6f75726e616c2e7265736f6c7574696f6e2e686f6d65 = native journal resolution failed: home unavailable

m6a6f75726e616c2e636c692e6a6f75726e616c2e726f6f742e696e7370656374 = native journal project root resolution failed: could not inspect current executable: { $error }

m6a6f75726e616c2e636c692e6a6f75726e616c2e726f6f742e7061636b616765 = native journal project root resolution failed: could not locate source checkout or installed { "solstone" } package from { $path }

m6a6f75726e616c2e636c692e6f66666c6f61642e67756964616e63652e6368616e6765642e7265747279 = offload stopped because your journal changed while it was running. nothing was removed from this computer. run it again.

m6a6f75726e616c2e636c692e6f66666c6f61642e67756964616e63652e6368616e6765642e726576696577 = offload stopped because your journal changed while it was running. nothing was removed from this computer. keep offload's recovery files and review the recorded effects before retrying.

m6a6f75726e616c2e636c692e6f66666c6f61642e67756964616e63652e636c6f636b = the operation clock could not be confirmed.

m6a6f75726e616c2e636c692e6f66666c6f61642e67756964616e63652e6a6f75726e616c = your journal could not be confirmed. keep its recovery files for review.

m6a6f75726e616c2e636c692e6f66666c6f61642e67756964616e63652e706c6174666f726d = offload execution is unavailable on this platform.

m6a6f75726e616c2e636c692e6f66666c6f61642e67756964616e63652e7374617274 = offload did not start.

m6a6f75726e616c2e636c692e6f66666c6f61642e67756964616e63652e746f6f6c = use the installed { "solstone" } command with its packaged backup tool.

m6a6f75726e616c2e636c692e6f66666c6f61642e67756964616e63652e756e66696e6973686564 = offload did not finish. keep its recovery files and review the recorded effects before retrying.

m6a6f75726e616c2e636c692e6f66666c6f61642e726573756c742e617263686976652e6661696c6564 = the archive step of the offload didn't complete

m6a6f75726e616c2e636c692e6f66666c6f61642e726573756c742e6261636b75702e6661696c696e67 = your last backup didn't complete successfully

m6a6f75726e616c2e636c692e6f66666c6f61642e726573756c742e6261636b75702e6e6f742e7265616479 = your backup isn't set up or hasn't completed a successful run yet

m6a6f75726e616c2e636c692e6f66666c6f61642e726573756c742e6275646765742e616c72656164792e736174697366696564 = the offload budget was already met, so nothing needed to be marked for release

m6a6f75726e616c2e636c692e6f66666c6f61642e726573756c742e6275646765742e736174697366696564 = enough media has been marked for release to meet the offload budget

m6a6f75726e616c2e636c692e6f66666c6f61642e726573756c742e636f6e6669726d2e6661696c6564 = the offload couldn't confirm the archived files actually reached the backup

m6a6f75726e616c2e636c692e6f66666c6f61642e726573756c742e636f6e6669726d2e746f6f6c2e6661696c6564 = the check that confirms an offload archive couldn't complete

m6a6f75726e616c2e636c692e6f66666c6f61642e726573756c742e6d61726b61626c652e6d656469612e657868617573746564 = every eligible file was already checked; nothing more can be marked for release right now

m6a6f75726e616c2e636c692e6f66666c6f61642e726573756c742e6f74686572 = stopped without a reason ({ $reason })

m6a6f75726e616c2e636c692e6f66666c6f61642e726573756c742e756e65787065637465642e6572726f72 = something unexpected interrupted the offload

m6a6f75726e616c2e636c692e6f66666c6f61642e726573756c742e766572696669636174696f6e2e6661696c6564 = the last backup verification found a problem with the backup's integrity

m6a6f75726e616c2e636c692e6f66666c6f61642e726573756c742e766572696669636174696f6e2e6d697373696e67 = no backup verification has completed yet

m6a6f75726e616c2e636c692e6f66666c6f61642e726573756c742e766572696669636174696f6e2e6f766572647565 = backup verification hasn't succeeded recently enough

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e617263686976652e63616e63656c6c6564 = offload stopped: the archive operation was cancelled.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e617263686976652e636c6f636b2e6368616e676564 = offload stopped: the archive clock moved backward.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e617263686976652e636f6e66696775726174696f6e2e6368616e676564 = offload stopped: archive configuration changed.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e617263686976652e636f6e66696775726174696f6e2e756e617661696c61626c65 = offload stopped: archive configuration could not be read.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e617263686976652e63726564656e7469616c732e65787069726564 = offload stopped: archive credentials expired.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e617263686976652e646561646c696e652e6578636565646564 = offload stopped: the archive operation deadline was reached.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e617263686976652e696e76616c69642e63726564656e7469616c2e657870697279 = offload stopped: archive credentials have an invalid expiry.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e617263686976652e696e76616c69642e73656c656374696f6e = offload stopped: archive sources must be a nonempty selection of absolute paths.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e617263686976652e6a6f75726e616c2e6964656e746974792e6368616e676564 = offload stopped: your journal could not be confirmed.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e617263686976652e6e6f742e636f6e66696775726564 = offload stopped: archive backup is not configured.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e617263686976652e7265706f7369746f72792e6964656e746974792e6368616e676564 = offload stopped: the backup no longer matches.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e617263686976652e7265706f7369746f72792e6964656e746974792e696e76616c6964 = offload stopped: the backup could not be identified.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e617263686976652e736e617073686f742e6964656e746974792e696e76616c6964 = offload stopped: the archive did not return a full snapshot identity.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e617263686976652e736f757263652e756e617661696c61626c65 = offload stopped: the prepared archive files could not be confirmed.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e617263686976652e746f6f6c2e756e617661696c61626c65 = offload stopped: an archive tool is unavailable.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e6372656174696f6e = offload stopped: making proposals did not finish.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e696e707574732e696f = offload stopped: a file in your journal couldn't be opened or read.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e696e707574732e7370616365 = offload stopped: not enough free space on this computer ({ $available } free, { $required } needed).

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e6e61746976652e7374617274 = offload stopped: archive start refused (status { $status }).

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e706c616e2e696e636f6d706c657465 = offload stopped: preparing proposals did not finish.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e706c616e6e696e67 = offload stopped: offload couldn't prepare its proposals.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e726561646261636b = offload stopped: reading the copy back from your backup failed.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e726561646261636b2e63726564656e7469616c732e756e617661696c61626c65 = offload stopped: archive read credentials are unavailable.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e726561646261636b2e696e636f6d706c6574652e646973636f76657279 = offload stopped: the archive walk did not account for every node.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e726561646261636b2e696e76616c69642e73656c656374696f6e = offload stopped: the selected archive file is invalid.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e726561646261636b2e6e61746976652e6661696c757265 = offload stopped: the archived file could not be prepared.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e726561646261636b2e6e6f742e636f6e66696775726564 = offload stopped: reading archived files is not configured.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e726561646261636b2e736e617073686f742e6e6f64652e756e617661696c61626c65 = offload stopped: the archived file could not be confirmed.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e726561646261636b2e7461726765742e726f6f742e6368616e676564 = offload stopped: the file could not be prepared in your journal.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e726561646261636b2e746f6f6c2e6368616e676564 = offload stopped: the installed archive tool changed; try again.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e726561646261636b2e746f6f6c2e6661696c6564 = offload stopped: the archive read command failed.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e726561646261636b2e746f6f6c2e756e617661696c61626c65 = offload stopped: the installed archive tool is unavailable.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e726561646261636b2e756e737570706f72746564 = offload stopped: reading archived files is unavailable on this platform.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e726561646261636b2e756e737570706f727465642e6261636b656e64 = offload stopped: this backup destination does not support reading archived files.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e726561646261636b2e756e737570706f727465642e746f6f6c2e76657273696f6e = offload stopped: the installed archive tool version is unsupported.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e7265636f7264 = offload stopped: saving offload's record failed.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e73656c656374696f6e = offload stopped: the selected recordings could not be confirmed.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e74656d706f726172792e636f7079 = offload stopped: making a temporary copy failed.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e746f6f6c2e726561736f6e = offload stopped: { $reason }.

m6a6f75726e616c2e636c692e6f66666c6f61642e73746f707065642e756e66696e69736865642e776f726b = offload stopped: your journal still has unfinished work.

m6a6f75726e616c2e636c692e706174682e637265617465 = could not create journal directory ({ $source }): { $path }: { $error }

m6a6f75726e616c2e636c692e706174682e686f6d65 = journal-path failed: could not determine home directory

m6a6f75726e616c2e636c692e706174682e75746638 = journal-path failed: config is not valid { "UTF-8" }

m6a6f75726e616c2e636c692e726570726f636573732e656d7074792e72616e6765 = no data for days { $day } through { $through }

m6a6f75726e616c2e636c692e726570726f636573732e66616365742e636c617373696669636174696f6e = { $path }: invalid facet classification

m6a6f75726e616c2e636c692e726570726f636573732e66616365742e75746638 = non-{ "UTF8" } facet

m6a6f75726e616c2e636c692e726570726f636573732e68656c70 = usage: { "solstone journal reprocess [-h] [--through THROUGH] [--yes] [--unit UNIT]" }{ "\u000a" }{ "                         [--facet FACET] [--from-scratch | --mark-updated |" }{ "\u000a" }{ "                         --owed] [--json] [-v] [-d]" }{ "\u000a" }{ "                         day" }{ "\u000a" }{ "\u000a" }Submit a past journal day for reprocessing{ "\u000a" }{ "\u000a" }positional arguments:{ "\u000a" }{ "  day                " }Past day in { "YYYYMMDD" } format{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  " }{ "-h, --help" }{ "         " }show this help message and exit{ "\u000a" }{ "  " }{ "--through THROUGH" }{ "  " }Inclusive range end in { "YYYYMMDD" } format{ "\u000a" }{ "  --yes" }{ "\u000a" }{ "  " }{ "--unit UNIT" }{ "        " }Request a retry of one failed daily processing step{ "\u000a" }{ "  " }{ "--facet FACET" }{ "      " }Facet of that unit; required when the unit has a facet{ "\u000a" }{ "  " }{ "--from-scratch" }{ "     " }Force a full daily re-run, preserving markers (does not{ "\u000a" }                     flag the day as updated){ "\u000a" }{ "  " }{ "--mark-updated" }{ "     " }Flag the day as having new raw data so daily processing{ "\u000a" }                     re-queues it, then nudge a drain{ "\u000a" }{ "  " }{ "--owed" }{ "             " }List the daily outputs owed on the day or range and why,{ "\u000a" }                     without submitting anything{ "\u000a" }{ "  " }{ "--json" }{ "             " }With { "--owed" }, report each day as states and counts: its{ "\u000a" }                     markers, its coverage, how many daily outputs it has and{ "\u000a" }                     how many are owed, by cause{ "\u000a" }{ "  " }{ "-v, --verbose" }{ "      " }Enable verbose output{ "\u000a" }{ "  " }{ "-d, --debug" }{ "        " }Enable debug logging

m6a6f75726e616c2e636c692e726570726f636573732e696e7075742e646179 = invalid day: { $day }

m6a6f75726e616c2e636c692e726570726f636573732e696e74656e742e62757379 = requested day is owned by another executor

m6a6f75726e616c2e636c692e726570726f636573732e696e74656e742e696e76616c69642e646179 = invalid work day

m6a6f75726e616c2e636c692e726570726f636573732e696e74656e742e756e6365727461696e = work intent publication is uncertain: { $outcome }

m6a6f75726e616c2e636c692e726570726f636573732e696e74656e742e756e7265636f676e697a6564 = unrecognized work intent

m6a6f75726e616c2e636c692e726570726f636573732e6e61746976652e617267756d656e7473 = native { "solstone-core" } { $verb } failed: arguments are not valid { "UTF-8" }

m6a6f75726e616c2e636c692e726570726f636573732e6e6f2e64617461 = no data for day { $day }

m6a6f75726e616c2e636c692e726570726f636573732e6e6f2e656e67696e65 = no model is chosen yet. choose one in thinking, then retry

m6a6f75726e616c2e636c692e726570726f636573732e6f7765642e696e7465726e616c = { "--owed" } only reports; use { "run_cli" }

m6a6f75726e616c2e636c692e726570726f636573732e6f7765642e6f6e6c79 = { "--owed" } only reports; nothing was submitted

m6a6f75726e616c2e636c692e726570726f636573732e6f7765642e73756d6d617279 = { $i18nCategory ->
    [one] { $i18nCategory-days_quantity ->
        [one] { $i18nCategory-unreadable_quantity ->
            [one] { $owed } owed output(s) on { $days } past day(s); { $unreadable } unreadable day(s)
           *[other] { $owed } owed output(s) on { $days } past day(s); { $unreadable } unreadable day(s)
        }
       *[other] { $i18nCategory-unreadable_quantity ->
            [one] { $owed } owed output(s) on { $days } past day(s); { $unreadable } unreadable day(s)
           *[other] { $owed } owed output(s) on { $days } past day(s); { $unreadable } unreadable day(s)
        }
    }
   *[other] { $i18nCategory-days_quantity ->
        [one] { $i18nCategory-unreadable_quantity ->
            [one] { $owed } owed output(s) on { $days } past day(s); { $unreadable } unreadable day(s)
           *[other] { $owed } owed output(s) on { $days } past day(s); { $unreadable } unreadable day(s)
        }
       *[other] { $i18nCategory-unreadable_quantity ->
            [one] { $owed } owed output(s) on { $days } past day(s); { $unreadable } unreadable day(s)
           *[other] { $owed } owed output(s) on { $days } past day(s); { $unreadable } unreadable day(s)
        }
    }
}

m6a6f75726e616c2e636c692e726570726f636573732e6f7765642e73756d6d6172792e636175736573 = { $i18nCategory ->
    [one] { $i18nCategory-days_quantity ->
        [one] { $i18nCategory-unreadable_quantity ->
            [one] { $owed } owed output(s) on { $days } past day(s): { $summary }; { $unreadable } unreadable day(s)
           *[other] { $owed } owed output(s) on { $days } past day(s): { $summary }; { $unreadable } unreadable day(s)
        }
       *[other] { $i18nCategory-unreadable_quantity ->
            [one] { $owed } owed output(s) on { $days } past day(s): { $summary }; { $unreadable } unreadable day(s)
           *[other] { $owed } owed output(s) on { $days } past day(s): { $summary }; { $unreadable } unreadable day(s)
        }
    }
   *[other] { $i18nCategory-days_quantity ->
        [one] { $i18nCategory-unreadable_quantity ->
            [one] { $owed } owed output(s) on { $days } past day(s): { $summary }; { $unreadable } unreadable day(s)
           *[other] { $owed } owed output(s) on { $days } past day(s): { $summary }; { $unreadable } unreadable day(s)
        }
       *[other] { $i18nCategory-unreadable_quantity ->
            [one] { $owed } owed output(s) on { $days } past day(s): { $summary }; { $unreadable } unreadable day(s)
           *[other] { $owed } owed output(s) on { $days } past day(s): { $summary }; { $unreadable } unreadable day(s)
        }
    }
}

m6a6f75726e616c2e636c692e726570726f636573732e706173742e6f6e6c79 = reprocess is past-only (cannot reprocess today or a future day)

m6a6f75726e616c2e636c692e726570726f636573732e706174682e75746638 = non-{ "UTF8" } source path

m6a6f75726e616c2e636c692e726570726f636573732e72616e67652e706c616e = { $i18nCategory ->
    [one] { $i18nCategory-segments_quantity ->
        [one] from-scratch reprocess plan:{ "\u000a" }{ $days } day with data ({ $segments } segment) will be queued. Progress will be visible in { "solstone journal top" } or { "solstone journal health" }. Requests survive a supervisor restart; a day that cannot finish from scratch finishes as an ordinary reprocess.{ "\u000a" }This day can take hours; today's work goes first between tasks.{ "\u000a" }re-run with { "--yes" } to proceed
       *[other] from-scratch reprocess plan:{ "\u000a" }{ $days } day with data ({ $segments } segments) will be queued. Progress will be visible in { "solstone journal top" } or { "solstone journal health" }. Requests survive a supervisor restart; a day that cannot finish from scratch finishes as an ordinary reprocess.{ "\u000a" }This day can take hours; today's work goes first between tasks.{ "\u000a" }re-run with { "--yes" } to proceed
    }
   *[other] { $i18nCategory-segments_quantity ->
        [one] from-scratch reprocess plan:{ "\u000a" }{ $days } days with data ({ $segments } segment) will be queued. Progress will be visible in { "solstone journal top" } or { "solstone journal health" }. Requests survive a supervisor restart; a day that cannot finish from scratch finishes as an ordinary reprocess.{ "\u000a" }These days can take hours; today's work goes first between tasks.{ "\u000a" }re-run with { "--yes" } to proceed
       *[other] from-scratch reprocess plan:{ "\u000a" }{ $days } days with data ({ $segments } segments) will be queued. Progress will be visible in { "solstone journal top" } or { "solstone journal health" }. Requests survive a supervisor restart; a day that cannot finish from scratch finishes as an ordinary reprocess.{ "\u000a" }These days can take hours; today's work goes first between tasks.{ "\u000a" }re-run with { "--yes" } to proceed
    }
}

m6a6f75726e616c2e636c692e726570726f636573732e72616e67652e7361766564 = { $i18nCategory ->
    [one] { $i18nCategory-segments_quantity ->
        [one] saved from-scratch requests for { $days } day ({ $segments } segment){ "\u000a" }progress is visible in health; today's work goes first
       *[other] saved from-scratch requests for { $days } day ({ $segments } segments){ "\u000a" }progress is visible in health; today's work goes first
    }
   *[other] { $i18nCategory-segments_quantity ->
        [one] saved from-scratch requests for { $days } days ({ $segments } segment){ "\u000a" }progress is visible in health; today's work goes first
       *[other] saved from-scratch requests for { $days } days ({ $segments } segments){ "\u000a" }progress is visible in health; today's work goes first
    }
}

m6a6f75726e616c2e636c692e726570726f636573732e7265636f72642e696e76616c6964 = Malformed daily unit record: invalid daily unit identity

m6a6f75726e616c2e636c692e726570726f636573732e7265636f72642e696f = I/O error: { $cause }

m6a6f75726e616c2e636c692e726570726f636573732e7265636f72642e6d69736d61746368 = Malformed daily unit record: identity mismatch in record at { $path }: expected { $expected }, found { $found }

m6a6f75726e616c2e636c692e726570726f636573732e72657472792e6163636570746564 = this daily output is already accepted

m6a6f75726e616c2e636c692e726570726f636573732e72657472792e64697361626c6564 = daily talent is no longer enabled

m6a6f75726e616c2e636c692e726570726f636573732e72657472792e696e76616c69642e646179 = invalid retry day

m6a6f75726e616c2e636c692e726570726f636573732e72657472792e696e76616c69642e756e6974 = invalid retry unit

m6a6f75726e616c2e636c692e726570726f636573732e72657472792e6d697373696e67 = daily unit is missing

m6a6f75726e616c2e636c692e726570726f636573732e72657472792e6d697373696e672e65766964656e6365 = missing held evidence

m6a6f75726e616c2e636c692e726570726f636573732e72657472792e6e6f2e6c6f6e6765722e68656c64 = unit is no longer held

m6a6f75726e616c2e636c692e726570726f636573732e72657472792e6e6f742e6461696c79 = not daily work

m6a6f75726e616c2e636c692e726570726f636573732e72657472792e7374617465 = this daily unit cannot be retried without resolving its current state

m6a6f75726e616c2e636c692e726570726f636573732e7363686564756c652e6c6f6f6b6261636b = daily schedule lookback outside { "1..=366" }

m6a6f75726e616c2e636c692e726570726f636573732e736368656d612e6d697373696e67 = schema prompt path is missing

m6a6f75726e616c2e636c692e726570726f636573732e736368656d612e6e616d65 = schema must be a filename

m6a6f75726e616c2e636c692e726570726f636573732e73686170652e696e76616c6964 = invalid shape for { $path }

m6a6f75726e616c2e636c692e726570726f636573732e73686170652e6f626a656374 = shape sidecar is not an object

m6a6f75726e616c2e636c692e726570726f636573732e73686170652e7772697474656e = invalid written source shape: { $path }

m6a6f75726e616c2e636c692e726570726f636573732e736f757263652e6a736f6e2e6f626a656374 = { $path } is not a { "JSON" } object

m6a6f75726e616c2e636c692e726570726f636573732e736f757263652e6c696e652e6572726f72 = { $path } line { $line }: { $error }

m6a6f75726e616c2e636c692e726570726f636573732e736f757263652e6c696e652e6f626a656374 = { $path } line { $line } is not an object

m6a6f75726e616c2e636c692e726570726f636573732e736f757263652e73796d6c696e6b = daily source is a symbolic link: { $path }

m6a6f75726e616c2e636c692e726570726f636573732e736f757263652e75746638 = non-{ "UTF8" } daily source path

m6a6f75726e616c2e636c692e726570726f636573732e7375626d69747465642e66726f6d2e73637261746368 = reprocess (from-scratch) submitted for { $day }

m6a6f75726e616c2e636c692e726570726f636573732e7375626d69747465642e6d61726b2e75706461746564 = reprocess (mark-updated) submitted for { $day }

m6a6f75726e616c2e636c692e726570726f636573732e7375626d69747465642e70726f636573732e6e6f77 = reprocess (process-now) submitted for { $day }

m6a6f75726e616c2e636c692e726570726f636573732e74616c656e742e6f7665727269646573 = failed to read talent overrides from { $path }

m6a6f75726e616c2e636c692e726570726f636573732e74656d706c6174652e75746638 = non-{ "UTF8" } template name

m6a6f75726e616c2e636c692e726570726f636573732e7468726f7567682e6f72646572 = { "--through" } must be on or after the start day

m6a6f75726e616c2e636c692e726570726f636573732e7468726f7567682e7265717569726564 = { "--through" } requires { "--from-scratch" }

m6a6f75726e616c2e636c692e726570726f636573732e756e69742e6661696c6564 = reprocess unit failed: { $error }

m6a6f75726e616c2e636c692e726570726f636573732e756e69742e7361766564 = retry request saved for { $unit } on { $day }

m6a6f75726e616c2e636c692e726570726f636573732e756e7265616461626c65 = { $day }  (unreadable: { $error })

m6a6f75726e616c2e636c692e726570726f636573732e75736167652e636f6e666c696374 = usage: { "solstone journal reprocess [-h] [--through THROUGH] [--yes] [--unit UNIT]" }{ "\u000a" }{ "                         [--facet FACET] [--from-scratch | --mark-updated |" }{ "\u000a" }{ "                         --owed] [--json] [-v] [-d]" }{ "\u000a" }{ "                         day" }{ "\u000a" }{ "solstone journal reprocess" }: error: argument { $option }: not allowed with argument { $previous }

m6a6f75726e616c2e636c692e726570726f636573732e75736167652e6d697373696e672e646179 = usage: { "solstone journal reprocess [-h] [--through THROUGH] [--yes] [--unit UNIT]" }{ "\u000a" }{ "                         [--facet FACET] [--from-scratch | --mark-updated |" }{ "\u000a" }{ "                         --owed] [--json] [-v] [-d]" }{ "\u000a" }{ "                         day" }{ "\u000a" }{ "solstone journal reprocess" }: error: the following arguments are required: { $positional }

m6a6f75726e616c2e636c692e726570726f636573732e75736167652e6d697373696e672e76616c7565 = usage: { "solstone journal reprocess [-h] [--through THROUGH] [--yes] [--unit UNIT]" }{ "\u000a" }{ "                         [--facet FACET] [--from-scratch | --mark-updated |" }{ "\u000a" }{ "                         --owed] [--json] [-v] [-d]" }{ "\u000a" }{ "                         day" }{ "\u000a" }{ "solstone journal reprocess" }: error: argument { $option }: expected one argument

m6a6f75726e616c2e636c692e726570726f636573732e75736167652e7265717569726573 = usage: { "solstone journal reprocess [-h] [--through THROUGH] [--yes] [--unit UNIT]" }{ "\u000a" }{ "                         [--facet FACET] [--from-scratch | --mark-updated |" }{ "\u000a" }{ "                         --owed] [--json] [-v] [-d]" }{ "\u000a" }{ "                         day" }{ "\u000a" }{ "solstone journal reprocess" }: error: argument { $option }: requires { $required }

m6a6f75726e616c2e636c692e726570726f636573732e75736167652e756e6b6e6f776e = usage: { "solstone journal reprocess [-h] [--through THROUGH] [--yes] [--unit UNIT]" }{ "\u000a" }{ "                         [--facet FACET] [--from-scratch | --mark-updated |" }{ "\u000a" }{ "                         --owed] [--json] [-v] [-d]" }{ "\u000a" }{ "                         day" }{ "\u000a" }{ "solstone journal reprocess" }: error: unrecognized arguments: { $arguments }

m6a6f75726e616c2e636c692e726570726f636573732e77616974696e67 = request saved; start the background service with { "solstone journal up" } to process it

m6a6f75726e616c2e636c692e726574656e74696f6e2e70726f706f73616c2e656c696769626c65 = policy eligibility: Eligible { "{" } anchor: { $anchor }, age_days: { NUMBER($age_days, useGrouping: 0) }, period: Days({ NUMBER($period_days, useGrouping: 0) }) { "}" }

m6a6f75726e616c2e636c692e726574656e74696f6e2e70726f706f73616c2e666f7265766572 = policy eligibility: KeptForever

m6a6f75726e616c2e636c692e726574656e74696f6e2e70726f706f73616c2e6e6f2e616e63686f72 = policy eligibility: AnchorMissing { "{" } anchor: { $anchor } { "}" }

m6a6f75726e616c2e636c692e726574656e74696f6e2e70726f706f73616c2e756e7265636f72646564 = removal failed before a proposal was recorded

m6a6f75726e616c2e636c692e726574656e74696f6e2e70726f706f73616c2e796f756e67 = policy eligibility: TooYoung { "{" } anchor: { $anchor }, age_days: { NUMBER($age_days, useGrouping: 0) }, period: Days({ NUMBER($period_days, useGrouping: 0) }) { "}" }

m6a6f75726e616c2e636c692e726574656e74696f6e2e7265636f766572792e61637469766974792e62696e64696e67 = your journal couldn't be checked, so this delete stopped

m6a6f75726e616c2e636c692e726574656e74696f6e2e7265636f766572792e61637469766974792e62757379 = your journal is busy with another job

m6a6f75726e616c2e636c692e726574656e74696f6e2e7265636f766572792e61637469766974792e686f6c64 = another journal operation needs attention before this removal can continue

m6a6f75726e616c2e636c692e726574656e74696f6e2e7265636f766572792e61637469766974792e7265736572766564 = this location is reserved for a journal operation and cannot be removed

m6a6f75726e616c2e636c692e726574656e74696f6e2e7265636f766572792e706174682e6469726563746f7279 = there is a folder where a recording was expected

m6a6f75726e616c2e636c692e726574656e74696f6e2e7265636f766572792e706174682e696e76616c6964 = the location does not name a file inside your journal

m6a6f75726e616c2e636c692e726574656e74696f6e2e7265636f766572792e706174682e6f757473696465 = this is not inside your journal, so it was left alone

m6a6f75726e616c2e636c692e726574656e74696f6e2e7265636f766572792e706174682e7065726d697373696f6e = the file could not be read or removed, which usually means a permission problem

m6a6f75726e616c2e636c692e726574656e74696f6e2e7265636f766572792e706174682e756e617661696c61626c65 = the file could not be removed; the journal was left unchanged

m6a6f75726e616c2e636c692e726574656e74696f6e2e7265636f766572792e7265636f72642e70726570617265 = the record of this removal could not be prepared

m6a6f75726e616c2e636c692e726574656e74696f6e2e7265636f766572792e7265636f72642e7772697465 = the record of this removal could not be written

m6a6f75726e616c2e636c692e726574656e74696f6e2e7265636f766572792e7365676d656e742e62757379 = another process is working on this segment

m6a6f75726e616c2e636c692e726574656e74696f6e2e7265636f766572792e7365676d656e742e676f6e65 = this segment has already been removed

m6a6f75726e616c2e636c692e726574656e74696f6e2e7265636f766572792e7365676d656e742e6c6976652e6f63637570696564 = something new was written where this segment was, so it has been left set aside rather than overwritten

m6a6f75726e616c2e636c692e726574656e74696f6e2e7265636f766572792e7365676d656e742e6d697373696e67 = there is no such segment in your journal

m6a6f75726e616c2e636c692e726574656e74696f6e2e7265636f766572792e7365676d656e742e737461676564 = this segment is part-way through removal and is set aside; it is not listed with your other segments until it finishes

m6a6f75726e616c2e636c692e726574656e74696f6e2e7265636f766572792e7365676d656e742e756e636f6e6669726d6564 = this could not be confirmed gone, so it is not being reported as removed

m6a6f75726e616c2e636c692e726574656e74696f6e2e7265636f766572792e7365676d656e742e756e66696e6973686564 = a previous removal of this segment did not finish; it needs looking at before this can be retried

m6a6f75726e616c2e636c692e726574656e74696f6e2e72656d6f76616c2e756e6365727461696e = deletion couldn't be completed or confirmed. recovery is needed.

m6a6f75726e616c2e636c692e7365676d656e742e617267756d656e742e6d697373696e67 = usage: { "solstone journal segment <command> [options]" }{ "\u000a" }{ "solstone journal segment" }: error: unrecognized arguments: the following arguments are required: { $input }

m6a6f75726e616c2e636c692e7365676d656e742e617267756d656e742e756e6b6e6f776e = usage: { "solstone journal segment <command> [options]" }{ "\u000a" }{ "solstone journal segment" }: error: unrecognized arguments: { $arguments }

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e6261636b77617264 = backward chain

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e636861696e2e6e6f2e646179 = { "prev_segment" } set without { "prev_day" }

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e636861696e2e6e6f2e737563636573736f72 = next segment not found, not stream tail

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e636861696e2e70726576696f7573 = previous segment found

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e636861696e2e7374617274 = no previous segment

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e636861696e2e7461696c = stream tail

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e636f6e74656e742e656d707479 = no { "audio.jsonl" }, { "screen.jsonl" }, or { "browser_*.jsonl" }

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e636f6e74656e742e70726573656e74 = content files present

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e6469726563746f72792e616273656e74 = segment directory missing

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e6469726563746f72792e657869737473 = directory exists

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e6469726563746f72792e6d697373696e67 = directory missing

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e666f7277617264 = forward chain

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e696e646578 = index presence

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e696e6465782e616273656e74 = journal index not available

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e696e6465782e6572726f72 = journal index error: { $error } (run: { "solstone journal indexer --rescan" })

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e696e6465782e6d697373696e67 = segment not indexed

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e696e6465782e70726573656e74 = segment indexed

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e6d61726b65722e657869737473 = { "stream.json" } exists

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e6d61726b65722e696e76616c6964 = { "stream.json" } invalid { "JSON" }

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e6d61726b65722e6d697373696e67 = { "stream.json" } missing

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e6d61726b65722e6e6f2e73747265616d = { "stream.json" } missing stream field

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e6d61726b65722e76616c6964 = { "stream.json" } valid

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e6e657874 = next segment { $path }

m6a6f75726e616c2e636c692e7365676d656e742e636865636b2e70726576696f75732e6d697373696e67 = missing previous segment { $path }

m6a6f75726e616c2e636c692e7365676d656e742e6461792e656d707479 = No segments found for { $day }

m6a6f75726e616c2e636c692e7365676d656e742e67756172642e6163746976697479 = another journal operation needs attention before this change can continue

m6a6f75726e616c2e636c692e7365676d656e742e67756172642e62696e64696e67 = the journal could not be checked, so this change is on hold

m6a6f75726e616c2e636c692e7365676d656e742e67756172642e7265736572766564 = this location is reserved for a journal operation and cannot be changed

m6a6f75726e616c2e636c692e7365676d656e742e68656c702e696e7370656374 = usage: { "solstone journal segment <command> [options] inspect [-h] [--json] path" }{ "\u000a" }{ "\u000a" }positional arguments:{ "\u000a" }{ "  path        " }Segment path: { "day/stream/segment" } (e.g.{ "\u000a" }{ "              20260304/default/090000_300)" }{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  -h, --help  " }show this help message and exit{ "\u000a" }{ "  --json      " }Output as { "JSON" }

m6a6f75726e616c2e636c692e7365676d656e742e68656c702e6c697374 = usage: { "solstone journal segment <command> [options] list [-h] [--stream STREAM]" }{ "\u000a" }{ "                                                [--json]" }{ "\u000a" }{ "                                                day" }{ "\u000a" }{ "\u000a" }positional arguments:{ "\u000a" }{ "  day              " }Day in { "YYYYMMDD" } format{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  -h, --help       " }show this help message and exit{ "\u000a" }{ "  --stream STREAM  " }Filter to a specific stream{ "\u000a" }{ "  --json           " }Output as { "JSON" }

m6a6f75726e616c2e636c692e7365676d656e742e68656c702e6d6f7665 = usage: { "solstone journal segment <command> [options] move [-h] --to-day TO_DAY" }{ "\u000a" }{ "                                                [--to-time TO_TIME]" }{ "\u000a" }{ "                                                [--dry-run] [-v]" }{ "\u000a" }{ "                                                path" }{ "\u000a" }{ "\u000a" }positional arguments:{ "\u000a" }{ "  path               " }Segment path: { "day/stream/segment" } (e.g.{ "\u000a" }{ "                     20260304/default/090000_300)" }{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  -h, --help         " }show this help message and exit{ "\u000a" }{ "  --to-day TO_DAY    " }Destination day ({ "YYYYMMDD" }){ "\u000a" }{ "  --to-time TO_TIME  " }New time ({ "HHMMSS" }), preserving original duration{ "\u000a" }{ "  --dry-run          " }Show plan without making changes{ "\u000a" }{ "  -v, --verbose      " }Show detailed progress

m6a6f75726e616c2e636c692e7365676d656e742e68656c702e746f70 = usage: { "solstone journal segment <command> [options]" }{ "\u000a" }{ "\u000a" }Inspect and manage journal segments{ "\u000a" }{ "\u000a" }positional arguments:{ "\u000a" }{ "  {list,inspect,verify,move}" }{ "\u000a" }{ "    list                " }List segments for a day{ "\u000a" }{ "    inspect             " }Show segment metadata{ "\u000a" }{ "    verify              " }Verify segment integrity{ "\u000a" }{ "    move                " }Move segment to a different day/time{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  -h, --help            " }show this help message and exit{ "\u000a" }{ "  -v, --verbose         " }Enable verbose output{ "\u000a" }{ "  -d, --debug           " }Enable debug logging

m6a6f75726e616c2e636c692e7365676d656e742e68656c702e766572696679 = usage: { "solstone journal segment <command> [options] verify [-h] [--day DAY] [--json]" }{ "\u000a" }{ "                                                  [path]" }{ "\u000a" }{ "\u000a" }positional arguments:{ "\u000a" }{ "  path        " }Segment path: { "day/stream/segment" }{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  -h, --help  " }show this help message and exit{ "\u000a" }{ "  --day DAY   " }Verify all segments for a day{ "\u000a" }{ "  --json      " }Output as { "JSON" }

m6a6f75726e616c2e636c692e7365676d656e742e6964656e746974792e64656661756c74 = segment list refused: named stream directory "{ "_default" }" cannot be spelled as a record identity: { $path }

m6a6f75726e616c2e636c692e7365676d656e742e6964656e746974792e6475706c6963617465 = segment list refused: multiple segments share stream { $stream } key { $key }

m6a6f75726e616c2e636c692e7365676d656e742e6964656e746974792e656e636f64696e67 = segment list refused: segment path is not { "UTF-8" } representable: { $path }

m6a6f75726e616c2e636c692e7365676d656e742e6964656e746974792e72656675736564 = segment list refused: { $detail }

m6a6f75726e616c2e636c692e7365676d656e742e696e73706563742e636861696e = Chain:

m6a6f75726e616c2e636c692e7365676d656e742e696e73706563742e6576656e7473 = { $i18nCategory ->
    [one] Events: { $entries } entry
   *[other] Events: { $entries } entries
}

m6a6f75726e616c2e636c692e7365676d656e742e696e73706563742e6576656e74732e747261637473 = { $i18nCategory ->
    [one] Events: { $entries } entry ({ $tracts })
   *[other] Events: { $entries } entries ({ $tracts })
}

m6a6f75726e616c2e636c692e7365676d656e742e696e73706563742e66696c6573 = Files ({ $count }):

m6a6f75726e616c2e636c692e7365676d656e742e696e73706563742e696e6465782e6572726f72 = Index: error ({ $error }) - run: { "solstone journal indexer --rescan" }

m6a6f75726e616c2e636c692e7365676d656e742e696e73706563742e696e6465782e6d697373696e67 = Index: not-indexed

m6a6f75726e616c2e636c692e7365676d656e742e696e73706563742e696e6465782e756e617661696c61626c65 = Index: unavailable

m6a6f75726e616c2e636c692e7365676d656e742e696e73706563742e696e6465786564 = { $i18nCategory ->
    [one] Index: indexed ({ $chunks } chunk)
   *[other] Index: indexed ({ $chunks } chunks)
}

m6a6f75726e616c2e636c692e7365676d656e742e696e73706563742e6e657874 = next: { $next }

m6a6f75726e616c2e636c692e7365676d656e742e696e73706563742e6e6578742e6e6f6e65 = next: (none)

m6a6f75726e616c2e636c692e7365676d656e742e696e73706563742e6e6578742e7461696c = next: (tail)

m6a6f75726e616c2e636c692e7365676d656e742e696e73706563742e70617468 = Segment: { $path }

m6a6f75726e616c2e636c692e7365676d656e742e696e73706563742e70726576 = prev: { $previous }

m6a6f75726e616c2e636c692e7365676d656e742e696e73706563742e707265762e6d697373696e67 = prev: { $path } [MISSING]

m6a6f75726e616c2e636c692e7365676d656e742e696e73706563742e707265762e6e6f6e65 = prev: (none)

m6a6f75726e616c2e636c692e7365676d656e742e696e73706563742e73697a65 = Size: { $size }

m6a6f75726e616c2e636c692e7365676d656e742e696e73706563742e73747265616d = Stream:  { $stream }

m6a6f75726e616c2e636c692e7365676d656e742e696e73706563742e73747265616d2e736571 = Stream:  { $stream } (seq { $sequence })

m6a6f75726e616c2e636c692e7365676d656e742e696e73706563742e74616c656e7473 = Talents ({ $count }):

m6a6f75726e616c2e636c692e7365676d656e742e696e73706563742e74696d65 = Time:    { $range } ({ $seconds }s)

m6a6f75726e616c2e636c692e7365676d656e742e6c6973742e68656164696e672e647572 = DUR

m6a6f75726e616c2e636c692e7365676d656e742e6c6973742e68656164696e672e66696c6573 = FILES

m6a6f75726e616c2e636c692e7365676d656e742e6c6973742e68656164696e672e7365676d656e74 = SEGMENT

m6a6f75726e616c2e636c692e7365676d656e742e6c6973742e68656164696e672e73697a65 = SIZE

m6a6f75726e616c2e636c692e7365676d656e742e6c6973742e68656164696e672e73747265616d = STREAM

m6a6f75726e616c2e636c692e7365676d656e742e6c6973742e68656164696e672e74616c656e7473 = TALENTS

m6a6f75726e616c2e636c692e7365676d656e742e6c6973742e68656164696e672e74696d65 = TIME

m6a6f75726e616c2e636c692e7365676d656e742e6d697373696e67 = Segment not found: { $path }

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e6461792e666f726d6174 = Invalid { "--to-day" } format: { $to_day } (expected { "YYYYMMDD" })

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e6469726563746f72792e63726561746564 = created directory: { $path }

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e6469726563746f72792e657869737473 = Destination { $destination } already exists; no changes made

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e6469726563746f72792e6661696c6564 = step 1 directory move failed: { $error }; source and index remain authoritative

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e6469726563746f72792e6d6f766564 = moved directory: { $source } -> { $destination }

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e647279 = [dry run] No changes made

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e6576656e74732e6e6f6e65 = no { "events.jsonl" } to rewrite

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e6576656e74732e72657772697474656e = { $i18nCategory ->
    [one] rewrote { $count } { "events.jsonl" } line (day: { $source_day }->{ $destination_day }, segment: { $source }->{ $destination })
   *[other] rewrote { $count } { "events.jsonl" } lines (day: { $source_day }->{ $destination_day }, segment: { $source }->{ $destination })
}

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e657869737473 = Segment { $segment } already exists on { $day }/{ $stream }. Use { "--to-time" } to specify an alternate time.

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e6661696c2e616d626967756f7573 = { $i18nCategory ->
    [one] step 3 successor patch inconsistent after move: ambiguous chain: { NUMBER($count, useGrouping: 0) } successor left unpatched
   *[other] step 3 successor patch inconsistent after move: ambiguous chain: { NUMBER($count, useGrouping: 0) } successors left unpatched
}

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e6661696c2e6576656e7473 = step 2 events rewrite inconsistent after move: { $detail }

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e6661696c2e6865616c74682e6d61726b6572 = step 6 health-marker touch inconsistent after move: { $detail }

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e6661696c2e736561726368 = step 5 search update inconsistent after move: the search update could not be queued: { $error }

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e6661696c2e737563636573736f72 = step 3 successor patch inconsistent after move: { $detail }

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e6661696c2e7461696c2e6c6f636b6564 = step 4 stream-tail repair inconsistent after move: could not lock stream record

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e6661696c2e7461696c2e6d616c666f726d6564 = step 4 stream-tail repair inconsistent after move: malformed stream record

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e6661696c2e7461696c2e6e6f2e7265636f7264 = step 4 stream-tail repair inconsistent after move: no stream record; identity was not re-authored

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e6661696c2e7461696c2e7772697465 = step 4 stream-tail repair inconsistent after move: atomic publication failed

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e6d61726b6572732e746f7563686564 = touched health markers: { $days }

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e6e6f2e6475726174696f6e = Source segment key has no duration

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e6e6f2e6d61726b6572 = No { "stream.json" } in source segment

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e6e6f2e736c6f74 = No available segment slot near { $destination_segment } on { $to_day }

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e706c616e = Move: { $source } -> { $destination }

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e706c616e2e616d626967756f7573 = { $i18nCategory ->
    [one] successor to patch: (ambiguous: { NUMBER($count, useGrouping: 0) } successor)
   *[other] successor to patch: (ambiguous: { NUMBER($count, useGrouping: 0) } successors)
}

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e706c616e2e6576656e7473 = { "events.jsonl" } lines: { $count }

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e706c616e2e696e646578 = index chunks: { $chunks }

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e706c616e2e696e6465782e6572726f72 = index read error: { $error } (delete+reindex will be attempted; run: { "solstone journal indexer --rescan" })

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e706c616e2e6d61726b657273 = health markers: { $source_day }, { $destination_day }

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e706c616e2e737563636573736f72 = successor to patch: { $path }

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e706c616e2e7461696c = successor to patch: (none - stream tail)

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e72756e6e696e67 = Executing move...

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e73616d65 = Source and destination are the same

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e7365617263682e717565756564 = search updates queued

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e73747265616d2e6d69736d61746368 = Stream mismatch: path says '{ $path_stream }' but { "stream.json" } says '{ $marker_stream }'

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e73747265616d2e72656275696c74 = rebuilt stream state: { $stream }

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e737563636573736f722e6e6f6e65 = no successor to patch (stream tail)

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e737563636573736f722e70617463686564 = patched successor { $path }

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e74616c656e7473 = { "think" } will re-run daily talents on the touched days

m6a6f75726e616c2e636c692e7365676d656e742e6d6f76652e74696d652e666f726d6174 = Invalid { "--to-time" } format: { $time } (expected { "HHMMSS" })

m6a6f75726e616c2e636c692e7365676d656e742e706174682e73796e746178 = Segment path must be { "day/stream/segment" } (e.g. { "20260304/default/090000_300" })

m6a6f75726e616c2e636c692e7365676d656e742e72656c6f636174696f6e2e64656c65746564 = segment was deleted by its owner

m6a6f75726e616c2e636c692e7365676d656e742e72656c6f636174696f6e2e6c6f636174696f6e = the segment location does not match the planned move

m6a6f75726e616c2e636c692e7365676d656e742e72656c6f636174696f6e2e6d61726b6572 = could not read marker

m6a6f75726e616c2e636c692e7365676d656e742e7374617475732e70617373 = PASS

m6a6f75726e616c2e636c692e7365676d656e742e7665726966792e6461792e726573756c74 = { $i18nCategory ->
    [one] Summary: { NUMBER($passed, useGrouping: 0) }/{ NUMBER($total, useGrouping: 0) } check passed
   *[other] Summary: { NUMBER($passed, useGrouping: 0) }/{ NUMBER($total, useGrouping: 0) } checks passed
}

m6a6f75726e616c2e636c692e7365676d656e742e7665726966792e696e707574 = { "verify" } requires a segment path or { "--day" }

m6a6f75726e616c2e636c692e7365676d656e742e7665726966792e726573756c74 = { $i18nCategory ->
    [one] { NUMBER($passed, useGrouping: 0) }/{ NUMBER($total, useGrouping: 0) } check passed
   *[other] { NUMBER($passed, useGrouping: 0) }/{ NUMBER($total, useGrouping: 0) } checks passed
}

m6a6f75726e616c2e636c692e73686172652e61637469766974696573 = could not read { $facet } activities for { $day }: { $detail }

m6a6f75726e616c2e636c692e73686172652e61637469766974792e6d697373696e67 = no activity '{ $activity }' in { $facet } on { $day }

m6a6f75726e616c2e636c692e73686172652e61637469766974792e756e6d617463686564 = activity '{ $activity }' does not match { $person }

m6a6f75726e616c2e636c692e73686172652e666163657473 = could not list facets: { $detail }

m6a6f75726e616c2e636c692e73686172652e6661696c757265 = { "solstone journal share" }: { $detail }

m6a6f75726e616c2e636c692e73686172652e68656c702e6d61746368 = usage: { "solstone journal share match [-h] --person PERSON --day YYYYMMDD [--facet FACET]" }{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  " }{ "--person PERSON" }{ "   " }entity id, or a unique name or alias of a person{ "\u000a" }{ "  " }{ "--day YYYYMMDD" }{ "    " }the day to read{ "\u000a" }{ "  " }{ "--facet FACET" }{ "     " }only this facet (default: every facet)

m6a6f75726e616c2e636c692e73686172652e68656c702e7061636b616765 = usage: { "solstone journal share package [-h] --person PERSON --day YYYYMMDD --facet FACET --activity ID" }{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  " }{ "--person PERSON" }{ "   " }entity id, or a unique name or alias of a person{ "\u000a" }{ "  " }{ "--day YYYYMMDD" }{ "    " }the activity's day{ "\u000a" }{ "  " }{ "--facet FACET" }{ "     " }the activity's facet{ "\u000a" }{ "  " }{ "--activity ID" }{ "     " }the activity id

m6a6f75726e616c2e636c692e73686172652e68656c702e726f6f74 = usage: { "solstone journal share [-h] {match,package} ..." }{ "\u000a" }{ "\u000a" }Find activities shared with a linked person and preview what would be shared.{ "\u000a" }Both subcommands only read the journal.{ "\u000a" }{ "\u000a" }commands:{ "\u000a" }{ "    match     " }List the day's activities that match a person, one { "JSON" } line each{ "\u000a" }{ "    package   " }Print the scoped summary package for one matching activity

m6a6f75726e616c2e636c692e73686172652e70617273652e617267756d656e74 = { "solstone journal share" }: error: unrecognized argument '{ $argument }'

m6a6f75726e616c2e636c692e73686172652e70617273652e6475706c6963617465 = { "solstone journal share" }: error: { $flag } given more than once

m6a6f75726e616c2e636c692e73686172652e70617273652e656e636f64696e67 = { "solstone journal share" }: error: arguments must be valid { "UTF-8" }

m6a6f75726e616c2e636c692e73686172652e70617273652e6d697373696e67 = { "solstone journal share" }: error: a subcommand is required

m6a6f75726e616c2e636c692e73686172652e70617273652e7265717569726564 = { "solstone journal share" }: error: { $option } is required

m6a6f75726e616c2e636c692e73686172652e70617273652e756e6b6e6f776e = { "solstone journal share" }: error: unknown subcommand '{ $argument }'

m6a6f75726e616c2e636c692e73686172652e70617273652e76616c7565 = { "solstone journal share" }: error: { $flag } needs a value

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e61637469766974792e756e617661696c61626c65 = activity summary unavailable: { $detail }

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e6368696c642e636c65616e7570 = { $primary } (cleanup: { $cleanup })

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e6368696c642e65786974 = exit { $code }

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e6368696c642e657869742e7369676e616c = exit { $code } signal { $signal }

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e6368696c642e696e76616c6964 = invalid child response ({ $status }): { $detail }; { "stdin_closed_early=" }{ $early }; { "stdout=" }{ $out }; { "stderr=" }{ $err }

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e6368696c642e70726f63657373 = process I/O failed: { $primary }; { "status=" }{ $status }; { "stdin_closed_early=" }{ $early }; { "stdout=" }{ $out }; { "stderr=" }{ $err }

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e6368696c642e70726f636573732e636c65616e7570 = process I/O failed: { $primary }; { "status=" }{ $status }; { "stdin_closed_early=" }{ $early }; { "stdout=" }{ $out }; { "stderr=" }{ $err }; { "cleanup=" }{ $cleanup }

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e6368696c642e70726f746f636f6c = protocol { $reason } ({ $status }): { $detail }; { "stdin_closed_early=" }{ $early }; { "stdout=" }{ $out }; { "stderr=" }{ $err }

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e6368696c642e7369676e616c = signal { $signal }

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e6368696c642e7374617475732e756e6b6e6f776e = status unknown

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e6368696c642e7472756e6361746564 = { $bytes } [truncated]

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e6368696c642e756e617661696c61626c65 = unavailable

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e6368696c642e756e6578706563746564 = unexpected child ({ $status }); { "stdin_closed_early=" }{ $early }; { "stdout=" }{ $out }; { "stderr=" }{ $err }

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e656e7469746965732e72656164 = could not read entities: { $detail }

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e656e746974792e696e76616c6964 = failed to load journal entity { $entity }: { $detail }

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e6d61746368696e67 = matching { $facet }/{ $day }/{ $activity } for { $entity }

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e6d6f64656c2e756e636f6e66696775726564 = No model is chosen yet. Choose one in Thinking.

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e706572736f6e2e616273656e74 = no person in this journal is named '{ $query }'

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e706572736f6e2e616d626967756f7573 = '{ $query }' names more than one person ({ $people }); use an entity id

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e70726573656e63652e756e617661696c61626c65 = presence check unavailable: { $detail }

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e7365676d656e742e756e617661696c61626c65 = segment summary unavailable: { $detail }

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e74616c656e742e696e76616c6964 = talent '{ $talent }' returned invalid { "JSON" }: { $error }

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e74616c656e742e70726570617265 = talent '{ $talent }': { $detail }

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e74616c656e742e74696d656f75742e6e756d65726963 = configuration '{ $talent }' declares a nonnumeric { "timeout_s" }

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e74616c656e742e74696d656f75742e72616e6765 = configuration '{ $talent }' declares { "timeout_s" } outside the supported range

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e74616c656e742e746f6b656e73 = talent '{ $talent }' declares no { "max_output_tokens" }

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e74616c656e742e74797065 = unsupported talent type: { $kind }

m6a6f75726e616c2e636c692e73686172652e70726f76696465722e74616c656e742e756e66696e6973686564 = talent '{ $talent }' did not finish: { $outcome }

m6a6f75726e616c2e636c692e73686172652e737461727475702e6661696c6564 = { "share" } failed: { $detail }

m6a6f75726e616c2e636c692e73686172652e737461727475702e72756e74696d652e63757272656e74 = could not resolve worker executable: { $error }

m6a6f75726e616c2e636c692e73686172652e737461727475702e72756e74696d652e706172656e74 = worker executable has no parent: { $path }

m6a6f75726e616c2e636c692e73686172652e737461727475702e72756e74696d652e726f6f74 = could not resolve installation root from executable directory: { $path }

m6a6f75726e616c2e636c692e73686172652e737461727475702e7369626c696e672e6d697373696e67 = { "share" } failed: missing sibling executable { $path }

m6a6f75726e616c2e636c692e73686172652e737461727475702e7369626c696e672e706172656e74 = { "share" } failed: current executable has no parent

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e6368696c642e696e7370656374 = inspect child before termination: { $detail }

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e6368696c642e72656170 = reap child: { $detail }

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e6368696c642e7265747279 = retry reap child: { $detail }

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e6368696c642e7465726d696e617465 = terminate child: { $detail }

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e636f6c6c6563746f72 = { $pipe } collector: { $detail }

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e636f6c6c6563746f722e70616e6963 = { $pipe } collector: thread panicked

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e6669656c642e6172726179 = { $field } must be an array or null

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e6669656c642e626f6f6c65616e = { $field } must be a boolean

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e6669656c642e6f7074696f6e616c = { $field } must be a string or null

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e6669656c642e737472696e67 = { $field } must be a string

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e6669656c642e737472696e6773 = { $field } must contain only strings

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e6f626a656374 = record must be a { "JSON" } object

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e6f7574636f6d65 = response outcome is not supported

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e706970652e6d697373696e67 = wire { $pipe } is unavailable

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e726571756573742e656d707479 = { "contents" } must be non-empty

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e726571756573742e6f766572666c6f77 = { "overflow_text" } requires the first content part to be text

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e736368656d61 = record schema is not supported

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e7374646f75742e6c696d6974 = { "stdout" } truncated at { $limit } bytes

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e7573616765 = { "usage" } is required

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e776f726b65722e6368616e6e656c = pipe worker completion channel: { $detail }

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e776f726b65722e6a6f696e = { $pipe } worker join: thread panicked

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e776f726b65722e7374617274 = start { $pipe } worker: { $detail }

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e777269746572 = { "stdin" } writer: { $detail }

m6a6f75726e616c2e636c692e73686172652e7472616e73706f72742e7772697465722e70616e6963 = { "stdin" } writer: thread panicked

m6a6f75726e616c2e636c692e73686172652e75736167652e6d61746368 = usage: { "solstone journal share match [-h] --person PERSON --day YYYYMMDD [--facet FACET]" }

m6a6f75726e616c2e636c692e73686172652e75736167652e7061636b616765 = usage: { "solstone journal share package [-h] --person PERSON --day YYYYMMDD --facet FACET --activity ID" }

m6a6f75726e616c2e636c692e73686172652e75736167652e726f6f74 = usage: { "solstone journal share [-h] {match,package} ..." }

m6a6f75726e616c2e636c692e736f6c2e68656c702e61707073 = Apps ({ "solstone call <app>" }):

m6a6f75726e616c2e636c692e736f6c2e68656c702e636f6d7075746572 = This computer

m6a6f75726e616c2e636c692e736f6c2e68656c702e6a6f75726e616c = Your journal

m6a6f75726e616c2e636c692e736f6c2e68656c702e7469746c65 = { "solstone" } - your journal, from the command line

m6a6f75726e616c2e636c692e736f6c2e68656c702e746f6f6c73 = Tools

m6a6f75726e616c2e636c692e736f6c2e6c61756e636865722e6379636c652e6d6163 = { "solstone" }: native launcher symlink cycle detected. Reinstall the journal app.

m6a6f75726e616c2e636c692e736f6c2e6c61756e636865722e6379636c652e756e6978 = { "solstone" }: native launcher symlink cycle detected. Reinstall { "solstone" } and { "solstone-core-sol" }.

m6a6f75726e616c2e636c692e736f6c2e6c61756e636865722e64616e676c696e672e6d6163 = { "solstone" }: native launcher symlink is dangling: { $path }. Reinstall the journal app.

m6a6f75726e616c2e636c692e736f6c2e6c61756e636865722e64616e676c696e672e756e6978 = { "solstone" }: native launcher symlink is dangling: { $path }. Reinstall { "solstone" } and { "solstone-core-sol" }.

m6a6f75726e616c2e636c692e736f6c2e6c61756e636865722e65786563757461626c652e6d6163 = { "solstone" }: native { "solstone-core-sol" } sibling is not executable: { $path }. Reinstall the journal app.

m6a6f75726e616c2e636c692e736f6c2e6c61756e636865722e65786563757461626c652e756e6978 = { "solstone" }: native { "solstone-core-sol" } sibling is not executable: { $path }. Reinstall { "solstone" } and { "solstone-core-sol" }.

m6a6f75726e616c2e636c692e736f6c2e6c61756e636865722e6d697373696e672e6d6163 = { "solstone" }: native { "solstone-core-sol" } sibling is missing: { $path }. Reinstall the journal app.

m6a6f75726e616c2e636c692e736f6c2e6c61756e636865722e6d697373696e672e756e6978 = { "solstone" }: native { "solstone-core-sol" } sibling is missing: { $path }. Reinstall { "solstone" } and { "solstone-core-sol" }.

m6a6f75726e616c2e636c692e736f6c2e6d6f766564 = '{ $command }' moved to '{ "solstone journal" } { $command }'; run that instead.{ "\u000a" }(commands that run the journal on this computer are under '{ "solstone journal" }'; see '{ "solstone journal --help" }'.)

m6a6f75726e616c2e636c692e736f6c2e7265736964656e742e706c6174666f726d = shutdown signals are unavailable on this platform

m6a6f75726e616c2e636c692e736f6c2e7265736964656e742e7369676e616c = native { "solstone" } resident signal setup failed: { $error }

m6a6f75726e616c2e636c692e736f6c2e7265736964656e742e73746172747570 = native { "solstone" } resident startup flush failed: { $error }

m6a6f75726e616c2e636c692e736f6c2e726f6f742e696e7370656374 = native { "solstone" } project root resolution failed: could not inspect current executable: { $error }

m6a6f75726e616c2e636c692e736f6c2e726f6f742e7061636b616765 = native { "solstone" } project root resolution failed: could not locate source checkout or installed { "solstone" } package from { $path }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6167656e742e696e76616c6964 = { "solstone skills" }: error: unsupported { "--agent" } { $agent }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6167656e742e7265717569726564 = { "solstone skills" }: error: { "--agent" } requires a value

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e617267756d656e742e696e76616c6964 = { "solstone skills" }: error: unrecognized argument { $argument }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e62756e646c652e6c696e6b6564 = bundled skill contains linked or special entries

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e636c65616e75702e6661696c6564 = refresh user skills: installed { $path }; old skill cleanup at { $backup } failed: { $error }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e636f6e6669672e6465636f6465 = config journal decode error: { $kind }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e636f6e6669672e6e6f742e6469726563746f7279 = { $path } is not a directory

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e636f70792e7370656369616c = special files are not permitted in user skill trees

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e636f70792e73796d6c696e6b = symlinks are not permitted in user skill trees

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6377642e6661696c6564 = { "solstone" }: native skills cwd is unavailable: { $error }. Run this command from an accessible directory.

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6578652e63757272656e74 = cannot determine current exe: { $error }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6578652e706172656e74 = current exe has no parent directory

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e66696c652e757466 = { "SKILL.md" } is not valid { "UTF-8" }: { $error }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e686f6d652e6162736f6c757465 = home must be an absolute path

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e686f6d652e6d697373696e67 = { "solstone" }: native skills home is unavailable. Set { "HOME" } to your home directory.

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6964656e746974792e63656e737573 = census read failed: { $detail }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6964656e746974792e686f6d652e6d7573742e62652e6162736f6c757465 = home must be absolute

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6964656e746974792e696e7374616c6c6174696f6e2e6964656e746974792e69732e756e737570706f727465642e666f722e616e64726f6964 = installation identity is unsupported for Android

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6964656e746974792e696f2e636c6f6e652e6e616d6573706163652e6469726563746f7279 = clone namespace directory: { $error }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6964656e746974792e696f2e636c6f6e652e6e616d6573706163652e7265676973747279 = clone namespace registry: { $error }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6964656e746974792e696f2e6f70656e2e686f6d652e6469726563746f7279 = open home directory: { $error }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6964656e746974792e696f2e6f70656e2e6e616d6573706163652e6469726563746f7279 = open namespace directory: { $error }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6964656e746974792e696f2e6f70656e2e6e616d6573706163652e7265676973747279 = open namespace registry: { $error }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6964656e746974792e696f2e726561642e6e616d6573706163652e7265676973747279 = read namespace registry: { $error }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6964656e746974792e696f2e737461742e73746f726167652e6469726563746f7279 = stat storage directory: { $error }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6964656e746974792e696f2e737461742e73746f726167652e66696c65 = stat storage file: { $error }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6964656e746974792e6e616d6573706163652e656e7472792e69732e6e6f742e6c6f776572636173652e686578 = namespace entry is not lowercase hex

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6964656e746974792e6e616d6573706163652e72656769737472792e656e74726965732e6c696d6974 = namespace registry exceeds 1024 entries

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6964656e746974792e6e616d6573706163652e72656769737472792e73697a652e6c696d6974 = namespace registry exceeds 16 MiB

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6964656e746974792e706174682e657363617065732e6974732e6162736f6c7574652e726f6f74 = path escapes its absolute root

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6964656e746974792e706174682e6d7573742e62652e6162736f6c7574652e616e642e6e756c2e66726565 = path must be absolute and NUL-free

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6964656e746974792e726f6f74 = cannot derive root token from { $path }: { $detail }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6964656e746974792e73746f726167652e6469726563746f72792e646f65732e6e6f742e686176652e707269766174652e6d6f6465 = storage directory does not have mode { "0700" }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6964656e746974792e73746f726167652e6469726563746f72792e69732e6e6f742e6f776e65642e62792e7468652e63757272656e742e75736572 = storage directory is not owned by the current user

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6964656e746974792e73746f726167652e66696c652e646f65732e6e6f742e686176652e707269766174652e6d6f6465 = storage file does not have mode { "0600" }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6964656e746974792e73746f726167652e66696c652e69732e6e6f742e6f776e65642e62792e7468652e63757272656e742e75736572 = storage file is not owned by the current user

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e696e7374616c6c2e646f6e65 = installed { $agent } { $skill } -> { $path }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6a6f75726e616c2e6162736f6c757465 = selected journal must be an absolute path

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6a6f75726e616c2e7265736f6c766564 = resolved journal path is not absolute

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6f7065726174696f6e2e6572726f72 = error: { $operation } { $path }: { $detail }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e6f776e65722e62617365 = cannot form owner base at { $path }: { $detail }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e706174682e6162736f6c757465 = path must be absolute

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e706174682e616e636573746f722e616273656e74 = no existing ancestor found for { $path }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e706174682e63616e6f6e6963616c = canonicalizing ancestor { $path } failed: { $error }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e706174682e696e7370656374 = inspecting ancestor { $path } failed: { $error }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e706174682e6f7665726c6170 = path { $path } overlaps protected journal

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e706174682e7265736f6c7665642e6f7665726c6170 = resolved ancestor path { $path } overlaps protected journal

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e7061796c6f61642e6661696c6564 = { "solstone" }: native skills payload root is unavailable: { $detail }. Reinstall { "solstone" } and { "solstone-core" }.

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e7061796c6f61642e70726f6a656374 = error: expected project skill at { $path }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e7061796c6f61642e756d6272656c6c61 = error: expected bundled umbrella skill at { "solstone/talent/solstone/SKILL.md" } ({ $path })

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e70726f6a6563742e6167656e74 = error: { "--agent" } { $agent } is not supported with { "--project" }; use { "--agent all" } or { "--agent claude" }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e70726f6a6563742e6d697373696e67 = expected project skill at { $path }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e7075626c6973682e726f6c6c6261636b = publishing skill failed: { $error }; restoring failed: { $restore }; backup path: { $path }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e72656769737472792e756e6b6e6f776e = installation registry is unknown

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e72656d6f76652e6469726563746f7279 = refusing to remove non-directory

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e72656d6f76652e646f6e65 = removed { $agent } { $skill } -> { $path }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e72656d6f76652e72657469726564 = removed { $agent } { $skill } (retired product skill) -> { $path }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e72656d6f76652e7374616c65 = removed { $agent } { $skill } (stale) -> { $path }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e72656d6f76652e73796d6c696e6b = refusing to remove non-symlink

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e7265706c6163652e646f6e65 = replaced { $agent } { $skill } -> { $path }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e736b69702e616273656e74 = skipped { $agent } { $skill } (nothing to remove)

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e736b69702e616c6c = no AI coding agent config directories found — skipping skill registration

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e736b69702e636f6e666967 = skipped { $agent } (config dir absent at { $path })

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e7374617475732e616273656e74 = { $agent } { $skill } not installed

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e7374617475732e686561646572 = agent      skill                state

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e7374617475732e696e7374616c6c6564 = { $agent } { $skill } installed

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e737562636f6d6d616e642e696e76616c6964 = { "solstone skills" }: error: unknown subcommand { $command_name }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e737562636f6d6d616e642e6d697373696e67 = { "solstone skills" }: error: missing subcommand

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e73796d6c696e6b2e756e737570706f72746564 = symlink creation unsupported

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e7461726765742e696e656c696769626c65 = target is ineligible for install

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e7461726765742e706172656e74 = target has no parent directory

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e7461726765742e707265736572766564 = target path overlaps protected journal or is preserved

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e747265652e6469726563746f7279 = skill tree requires an ordinary directory

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e747265652e6c696e6b6564 = linked entries are not permitted in user skill trees

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e747265652e7265666572656e6365 = bundled skill reference requires a directory with a regular { "SKILL.md" }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e747265652e7370656369616c = special entries are not permitted in user skill trees

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e7573616765 = Usage: { "solstone skills <install|uninstall|list> [args...]" }

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e7761726e696e672e6578697374696e67 = warning { $agent } { $skill } -> { $path } (existing content at target preserved)

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e7761726e696e672e72657469726564 = warning { $agent } { $skill } -> { $path } (does not match the retired { "sol" } skill; existing content at { $path } preserved)

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e7761726e696e672e7374616c65 = warning { $agent } { $skill } -> { $path } (existing content at stale target preserved)

m6a6f75726e616c2e636c692e736f6c2e736b696c6c732e7761726e696e6773 = Warnings:

m6a6f75726e616c2e636c692e736f6c2e756e737570706f72746564 = Unsupported native { "solstone" } command.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e617267756d656e742e6175746f6d617469632e636f6e666c696374 = { "--automatic" } and { "--unattended" } can only be combined with { "--json" }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e617267756d656e742e6175746f6d617469632e6d697373696e67 = { "--automatic" } requires { "on, off or status" }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e617267756d656e742e6175746f6d617469632e76616c7565 = { "--automatic" } takes { "on, off or status" }, not '{ $value }'

m6a6f75726e616c2e636c692e736f6c2e7570646174652e617267756d656e742e6c616e652e6d697373696e67 = { "--lane" } requires a value

m6a6f75726e616c2e636c692e736f6c2e7570646174652e617267756d656e742e6c616e652e75746638 = { "--lane" } must be valid { "utf-8" }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e617267756d656e742e6c616e652e76616c7565 = invalid release lane '{ $lane }'; choose { "release, staging, or dev" }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e617267756d656e742e6d6f64652e636f6e666c696374 = choose only one of { "--check" }, { "--hold" }, { "--unhold" }, or { "--rollback" }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e617267756d656e742e756e6b6e6f776e = unrecognized flag: { $flag }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e617267756d656e742e75746638 = arguments must be valid { "utf-8" }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e617267756d656e742e76657273696f6e2e636f6e666c696374 = { "--version" } cannot be combined with { "--check" }, { "--hold" }, or { "--unhold" }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e617267756d656e742e76657273696f6e2e666f726d6174 = invalid version format '{ $version }'; must be { "MAJOR.MINOR.PATCH" } or { "MAJOR.MINOR.PATCH-rN" }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e617267756d656e742e76657273696f6e2e6d697373696e67 = { "--version" } requires a value

m6a6f75726e616c2e636c692e736f6c2e7570646174652e617267756d656e742e76657273696f6e2e6e756d65726963 = invalid version format '{ $version }'; components must be numeric

m6a6f75726e616c2e636c692e736f6c2e7570646174652e617267756d656e742e76657273696f6e2e7265766973696f6e = invalid platform revision in '{ $version }'; revision must be numeric digits

m6a6f75726e616c2e636c692e736f6c2e7570646174652e617267756d656e742e76657273696f6e2e75746638 = { "--version" } must be valid { "utf-8" }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e617267756d656e742e7965732e636f6e666c696374 = { "--yes" } cannot be combined with { "--check" }, { "--hold" }, or { "--unhold" }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e63686f696365 = couldn't save your choice.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e686f6d65 = couldn't find your data directory.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e696e656c696769626c65 = this install can't update itself. automatic updates work on an install from { "https://solstone.app/install.sh" }.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e696e656c696769626c652e67656e65726963 = this install can't update itself.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e6c696e7578 = { "solstone update --automatic" } works on linux.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e6f6666 = automatic updates are off. to update, run { "solstone update" }.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e6f6e = automatic updates are on. your journal never restarts on its own; a new version starts the next time it does.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7061636b6167652e616464 = installed from a package. to get updates through your system's updater, add the { "solstone" } repository: { "solstone.app/install#linux-packages" }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e707265666978 = the install directory's name can't go in a service file.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e72756e_v75702d746f2d64617465 = automatic update: up to date

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e72756e_v696e7374616c6c6564 = automatic update: installed a new version

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e72756e_v6f6666 = automatic update: automatic updates are off

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e72756e_v706175736564 = automatic update: stopped after two tries that didn't work

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e72756e_v68656c64 = automatic update: updates are held

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e72756e_v686f6c642d756e6b6e6f776e = automatic update: the update hold can't be read

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e72756e_v6f66666c696e65 = automatic update: couldn't reach the update service

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e72756e_v6d657465726564 = automatic update: waiting for a connection that isn't metered

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e72756e_v77616974696e672d666f722d7369676e65642d696e7374616c6c6572 = automatic update: waiting for the next release

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e72756e_v62757379 = automatic update: another update was running

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e72756e_v6e6f742d656c696769626c65 = automatic update: this install can't update itself

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e72756e_v6661696c6564 = automatic update: the update didn't work ({ $code })

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e73657373696f6e = couldn't set up the daily update check. your systemd user session may not be running.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e737461747573_v6f6e = automatic updates: on

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e737461747573_v6f6666 = automatic updates: off

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e737461747573_v756e61736b6564 = automatic updates: not turned on

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e737461747573_v706175736564 = automatic updates: stopped after two tries that didn't work

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e636865636b6564_v6f6e = automatic updates: on · last checked { $date }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e636865636b6564_v6f6666 = automatic updates: off · last checked { $date }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e636865636b6564_v756e61736b6564 = automatic updates: not turned on · last checked { $date }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e636865636b6564_v706175736564 = automatic updates: stopped after two tries that didn't work · last checked { $date }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c74_v75702d746f2d64617465 = automatic updates: off · up to date

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c74_v696e7374616c6c6564 = automatic updates: off · installed a new version

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c74_v6f6666 = automatic updates: off · automatic updates are off

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c74_v706175736564 = automatic updates: off · stopped after two tries that didn't work

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c74_v68656c64 = automatic updates: off · updates are held

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c74_v686f6c642d756e6b6e6f776e = automatic updates: off · the update hold can't be read

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c74_v6f66666c696e65 = automatic updates: off · couldn't reach the update service

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c74_v6d657465726564 = automatic updates: off · waiting for a connection that isn't metered

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c74_v77616974696e672d666f722d7369676e65642d696e7374616c6c6572 = automatic updates: off · waiting for the next release

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c74_v62757379 = automatic updates: off · another update was running

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c74_v6e6f742d656c696769626c65 = automatic updates: off · this install can't update itself

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c74_v6661696c6564 = automatic updates: off · the update didn't work ({ $code })

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c742e636865636b6564_v75702d746f2d64617465 = automatic updates: off · last checked { $date } · up to date

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c742e636865636b6564_v696e7374616c6c6564 = automatic updates: off · last checked { $date } · installed a new version

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c742e636865636b6564_v6f6666 = automatic updates: off · last checked { $date } · automatic updates are off

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c742e636865636b6564_v706175736564 = automatic updates: off · last checked { $date } · stopped after two tries that didn't work

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c742e636865636b6564_v68656c64 = automatic updates: off · last checked { $date } · updates are held

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c742e636865636b6564_v686f6c642d756e6b6e6f776e = automatic updates: off · last checked { $date } · the update hold can't be read

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c742e636865636b6564_v6f66666c696e65 = automatic updates: off · last checked { $date } · couldn't reach the update service

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c742e636865636b6564_v6d657465726564 = automatic updates: off · last checked { $date } · waiting for a connection that isn't metered

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c742e636865636b6564_v77616974696e672d666f722d7369676e65642d696e7374616c6c6572 = automatic updates: off · last checked { $date } · waiting for the next release

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c742e636865636b6564_v62757379 = automatic updates: off · last checked { $date } · another update was running

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c742e636865636b6564_v6e6f742d656c696769626c65 = automatic updates: off · last checked { $date } · this install can't update itself

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f66662e726573756c742e636865636b6564_v6661696c6564 = automatic updates: off · last checked { $date } · the update didn't work ({ $code })

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c74_v75702d746f2d64617465 = automatic updates: on · up to date

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c74_v696e7374616c6c6564 = automatic updates: on · installed a new version

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c74_v6f6666 = automatic updates: on · automatic updates are off

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c74_v706175736564 = automatic updates: on · stopped after two tries that didn't work

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c74_v68656c64 = automatic updates: on · updates are held

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c74_v686f6c642d756e6b6e6f776e = automatic updates: on · the update hold can't be read

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c74_v6f66666c696e65 = automatic updates: on · couldn't reach the update service

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c74_v6d657465726564 = automatic updates: on · waiting for a connection that isn't metered

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c74_v77616974696e672d666f722d7369676e65642d696e7374616c6c6572 = automatic updates: on · waiting for the next release

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c74_v62757379 = automatic updates: on · another update was running

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c74_v6e6f742d656c696769626c65 = automatic updates: on · this install can't update itself

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c74_v6661696c6564 = automatic updates: on · the update didn't work ({ $code })

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c742e636865636b6564_v75702d746f2d64617465 = automatic updates: on · last checked { $date } · up to date

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c742e636865636b6564_v696e7374616c6c6564 = automatic updates: on · last checked { $date } · installed a new version

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c742e636865636b6564_v6f6666 = automatic updates: on · last checked { $date } · automatic updates are off

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c742e636865636b6564_v706175736564 = automatic updates: on · last checked { $date } · stopped after two tries that didn't work

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c742e636865636b6564_v68656c64 = automatic updates: on · last checked { $date } · updates are held

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c742e636865636b6564_v686f6c642d756e6b6e6f776e = automatic updates: on · last checked { $date } · the update hold can't be read

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c742e636865636b6564_v6f66666c696e65 = automatic updates: on · last checked { $date } · couldn't reach the update service

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c742e636865636b6564_v6d657465726564 = automatic updates: on · last checked { $date } · waiting for a connection that isn't metered

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c742e636865636b6564_v77616974696e672d666f722d7369676e65642d696e7374616c6c6572 = automatic updates: on · last checked { $date } · waiting for the next release

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c742e636865636b6564_v62757379 = automatic updates: on · last checked { $date } · another update was running

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c742e636865636b6564_v6e6f742d656c696769626c65 = automatic updates: on · last checked { $date } · this install can't update itself

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e6f6e2e726573756c742e636865636b6564_v6661696c6564 = automatic updates: on · last checked { $date } · the update didn't work ({ $code })

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c74_v75702d746f2d64617465 = automatic updates: stopped after two tries that didn't work · up to date

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c74_v696e7374616c6c6564 = automatic updates: stopped after two tries that didn't work · installed a new version

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c74_v6f6666 = automatic updates: stopped after two tries that didn't work · automatic updates are off

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c74_v706175736564 = automatic updates: stopped after two tries that didn't work · stopped after two tries that didn't work

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c74_v68656c64 = automatic updates: stopped after two tries that didn't work · updates are held

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c74_v686f6c642d756e6b6e6f776e = automatic updates: stopped after two tries that didn't work · the update hold can't be read

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c74_v6f66666c696e65 = automatic updates: stopped after two tries that didn't work · couldn't reach the update service

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c74_v6d657465726564 = automatic updates: stopped after two tries that didn't work · waiting for a connection that isn't metered

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c74_v77616974696e672d666f722d7369676e65642d696e7374616c6c6572 = automatic updates: stopped after two tries that didn't work · waiting for the next release

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c74_v62757379 = automatic updates: stopped after two tries that didn't work · another update was running

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c74_v6e6f742d656c696769626c65 = automatic updates: stopped after two tries that didn't work · this install can't update itself

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c74_v6661696c6564 = automatic updates: stopped after two tries that didn't work · the update didn't work ({ $code })

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c742e636865636b6564_v75702d746f2d64617465 = automatic updates: stopped after two tries that didn't work · last checked { $date } · up to date

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c742e636865636b6564_v696e7374616c6c6564 = automatic updates: stopped after two tries that didn't work · last checked { $date } · installed a new version

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c742e636865636b6564_v6f6666 = automatic updates: stopped after two tries that didn't work · last checked { $date } · automatic updates are off

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c742e636865636b6564_v706175736564 = automatic updates: stopped after two tries that didn't work · last checked { $date } · stopped after two tries that didn't work

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c742e636865636b6564_v68656c64 = automatic updates: stopped after two tries that didn't work · last checked { $date } · updates are held

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c742e636865636b6564_v686f6c642d756e6b6e6f776e = automatic updates: stopped after two tries that didn't work · last checked { $date } · the update hold can't be read

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c742e636865636b6564_v6f66666c696e65 = automatic updates: stopped after two tries that didn't work · last checked { $date } · couldn't reach the update service

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c742e636865636b6564_v6d657465726564 = automatic updates: stopped after two tries that didn't work · last checked { $date } · waiting for a connection that isn't metered

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c742e636865636b6564_v77616974696e672d666f722d7369676e65642d696e7374616c6c6572 = automatic updates: stopped after two tries that didn't work · last checked { $date } · waiting for the next release

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c742e636865636b6564_v62757379 = automatic updates: stopped after two tries that didn't work · last checked { $date } · another update was running

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c742e636865636b6564_v6e6f742d656c696769626c65 = automatic updates: stopped after two tries that didn't work · last checked { $date } · this install can't update itself

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e7061757365642e726573756c742e636865636b6564_v6661696c6564 = automatic updates: stopped after two tries that didn't work · last checked { $date } · the update didn't work ({ $code })

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c74_v75702d746f2d64617465 = automatic updates: not turned on · up to date

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c74_v696e7374616c6c6564 = automatic updates: not turned on · installed a new version

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c74_v6f6666 = automatic updates: not turned on · automatic updates are off

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c74_v706175736564 = automatic updates: not turned on · stopped after two tries that didn't work

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c74_v68656c64 = automatic updates: not turned on · updates are held

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c74_v686f6c642d756e6b6e6f776e = automatic updates: not turned on · the update hold can't be read

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c74_v6f66666c696e65 = automatic updates: not turned on · couldn't reach the update service

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c74_v6d657465726564 = automatic updates: not turned on · waiting for a connection that isn't metered

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c74_v77616974696e672d666f722d7369676e65642d696e7374616c6c6572 = automatic updates: not turned on · waiting for the next release

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c74_v62757379 = automatic updates: not turned on · another update was running

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c74_v6e6f742d656c696769626c65 = automatic updates: not turned on · this install can't update itself

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c74_v6661696c6564 = automatic updates: not turned on · the update didn't work ({ $code })

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c742e636865636b6564_v75702d746f2d64617465 = automatic updates: not turned on · last checked { $date } · up to date

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c742e636865636b6564_v696e7374616c6c6564 = automatic updates: not turned on · last checked { $date } · installed a new version

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c742e636865636b6564_v6f6666 = automatic updates: not turned on · last checked { $date } · automatic updates are off

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c742e636865636b6564_v706175736564 = automatic updates: not turned on · last checked { $date } · stopped after two tries that didn't work

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c742e636865636b6564_v68656c64 = automatic updates: not turned on · last checked { $date } · updates are held

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c742e636865636b6564_v686f6c642d756e6b6e6f776e = automatic updates: not turned on · last checked { $date } · the update hold can't be read

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c742e636865636b6564_v6f66666c696e65 = automatic updates: not turned on · last checked { $date } · couldn't reach the update service

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c742e636865636b6564_v6d657465726564 = automatic updates: not turned on · last checked { $date } · waiting for a connection that isn't metered

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c742e636865636b6564_v77616974696e672d666f722d7369676e65642d696e7374616c6c6572 = automatic updates: not turned on · last checked { $date } · waiting for the next release

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c742e636865636b6564_v62757379 = automatic updates: not turned on · last checked { $date } · another update was running

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c742e636865636b6564_v6e6f742d656c696769626c65 = automatic updates: not turned on · last checked { $date } · this install can't update itself

m6a6f75726e616c2e636c692e736f6c2e7570646174652e6175746f6d617469632e7374617475732e756e61736b65642e726573756c742e636865636b6564_v6661696c6564 = automatic updates: not turned on · last checked { $date } · the update didn't work ({ $code })

m6a6f75726e616c2e636c692e736f6c2e7570646174652e636174616c6f6775652e646f776e6c6f6164 = could not fetch the release catalogue.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e636174616c6f6775652e6b6579 = the release key in this build is unreadable.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e636174616c6f6775652e6c616e65 = unknown release lane: { $lane }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e636174616c6f6775652e6c6174657374 = the latest release marker is malformed.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e636174616c6f6775652e70696e = the release catalogue names a different release key.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e636174616c6f6775652e72656c65617365 = the release catalogue does not match the requested release.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e636174616c6f6775652e736368656d612e6261642e636174616c6f6775652e7265766973696f6e = the release catalogue is invalid: bad catalogue revision.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e636174616c6f6775652e736368656d612e6261642e6465736b746f702e76657273696f6e = the release catalogue is invalid: bad desktop version.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e636174616c6f6775652e736368656d612e6261642e696e7374616c6c6572 = the release catalogue is invalid: bad installer.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e636174616c6f6775652e736368656d612e6261642e76657273696f6e = the release catalogue is invalid: bad version.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e636174616c6f6775652e736368656d612e696e7374616c6c65722e696e2e616e2e756e737570706f727465642e736368656d61 = the release catalogue is invalid: installer in an unsupported schema.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e636174616c6f6775652e736368656d612e6b65792e6964 = the release catalogue is invalid: key id.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e636174616c6f6775652e736368656d612e6e6f2e6a6f75726e616c2e76657273696f6e = the release catalogue is invalid: no journal version.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e636174616c6f6775652e736368656d612e6e6f2e76657273696f6e = the release catalogue is invalid: no version.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e636174616c6f6775652e736368656d612e6e6f742e6a736f6e = the release catalogue is invalid: not json.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e636174616c6f6775652e736368656d612e756e737570706f727465642e70726f746f636f6c = the release catalogue is invalid: unsupported protocol.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e636174616c6f6775652e736368656d612e756e737570706f727465642e736368656d61 = the release catalogue is invalid: unsupported schema.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e636174616c6f6775652e7369676e6174757265 = the release catalogue failed its signature check.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e636174616c6f6775652e76657273696f6e = the requested platform version is invalid.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e68656c70 = usage: { "solstone update [--check] [--json] [--lane release|staging|dev] [--version VER] [--hold] [--unhold] [--rollback] [--yes] [--automatic on|off|status] [--help]" }{ "\u000a" }{ "\u000a" }update your installed { "solstone" } journal and apps.{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  " }{ "--check" }{ "      " }check whether an update is available without changing anything{ "\u000a" }{ "  " }{ "--json" }{ "       " }emit output as json{ "\u000a" }{ "  " }{ "--lane" }{ "       " }release lane to track ({ "release, staging, or dev" }){ "\u000a" }{ "  " }{ "--version" }{ "    " }specific target version to install{ "\u000a" }{ "  " }{ "--hold" }{ "       " }hold updates so a later update does not install them{ "\u000a" }{ "  " }{ "--unhold" }{ "     " }release update hold{ "\u000a" }{ "  " }{ "--rollback" }{ "   " }roll back to an earlier retained installation{ "\u000a" }{ "  " }{ "--yes" }{ "        " }accept unattended update prompts{ "\u000a" }{ "  " }{ "--automatic" }{ "  " }{ "on, off or status" }: whether your journal updates itself once a day{ "\u000a" }{ "  " }{ "--help" }{ "       " }show this help message

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726563656970742e636c61696d65642e7477696365 = component { $component } claimed by both tree and package receipts

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726563656970742e6475706c69636174652e726f6c6573 = both journal and cli components are recorded

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726563656970742e696e636f6d706c657465 = the installation receipt is incomplete or unsupported. use the original installer to recover it.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726563656970742e6b65792e6475706c6963617465 = duplicate key { $key }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726563656970742e6b65792e6f757473696465 = key outside section

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726563656970742e6c696e652e696e76616c6964 = malformed line: { $line }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726563656970742e6c696e652e70616972 = not key=value

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726563656970742e6e61746976652e66696c65 = native { "install-receipt" } is not a regular file

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726563656970742e6e61746976652e6d616c666f726d6564 = malformed native { "install-receipt" }: { $detail }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726563656970742e6e61746976652e726f757465 = native receipt route must be tree

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726563656970742e6e61746976652e736368656d61 = native receipt { "schema_version" } must be 1

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726563656970742e6e61746976652e76657273696f6e = installed version does not match receipt { "journal_version" }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726563656970742e7061636b6167652e66696c65 = package { "install.conf" } is not a regular file

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726563656970742e7061636b6167652e6d616c666f726d6564 = malformed package { "install.conf" }: { $detail }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726563656970742e73656374696f6e2e6475706c6963617465 = duplicate section [{ $section }]

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726563656970742e73656374696f6e2e6b65792e6475706c6963617465 = duplicate key { $key } in section [{ $section }]

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726563656970742e73656374696f6e2e756e636c6f736564 = unclosed section header

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726563656970742e73656374696f6e2e756e6b6e6f776e = unrecognized section in { "install.conf" }: { $section }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726563656970742e756e7265636f72646564 = this journal was installed from a package without the { "solstone" } installer, so { "solstone update" } can't update it yet. run { "curl -fsSL https://solstone.app/install.sh | sh -s -- --upgrade" } once. it checks the installed files against the signed release, and { "solstone update" } updates it after that.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726563656970742e757365722e66696c65 = user { "install.conf" } is not a regular file

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726563656970742e757365722e6d616c666f726d6564 = malformed user { "install.conf" }: { $detail }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e617661696c61626c65 = an update is available.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e617661696c61626c652e7061636b6167652e616464 = an update is available. installed from a package. to get updates through your system's updater, add the { "solstone" } repository: { "solstone.app/install#linux-packages" }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e617661696c61626c652e7061636b6167652e636f6e66696775726564 = an update is available. installed from a package. updates come through your system's updater.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e636174616c6f6775652e696e76616c6964 = the selected release metadata is invalid.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e636865636b6f7574 = this copy is a source checkout. install the released journal to update it.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e63757272656e74 = your journal is up to date.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e63757272656e742e7061636b6167652e616464 = your journal is up to date. installed from a package. to get updates through your system's updater, add the { "solstone" } repository: { "solstone.app/install#linux-packages" }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e63757272656e742e7061636b6167652e636f6e66696775726564 = your journal is up to date. installed from a package. updates come through your system's updater.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e68656c64 = updates are held. run { "solstone update --unhold" } to release them.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e686f6c642e6368616e676564 = the update hold cannot be read. nothing was changed.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e686f6c642e6f6666 = updates are no longer held.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e686f6c642e6f6e = updates are held.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e686f6c642e73617665 = could not save the update hold. check your data directory permissions.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e686f6c642e756e7265616461626c65 = the update hold cannot be read.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e696e7374616c6c2e6469726563746f7279 = the installation directory could not be identified.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e696e7374616c6c65722e617267756d656e7473 = invalid installer arguments.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e696e7374616c6c65722e6661696c6564 = installation did not succeed. use the original installer to recover.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e696e7374616c6c65722e6665746368 = could not fetch the installer.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e696e7374616c6c65722e72756e = could not run the installer.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e696e7374616c6c65722e746f6f6b6f766572 = updated your journal. it now updates with the { "solstone" } installer, from where it is.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e696e7374616c6c65722e75706461746564 = updated your journal.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e6c696e7578 = { "solstone update" } is available on linux.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e6d6f6465 = this mode is not an update.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e6f776e65722e756e6b6e6f776e = could not identify how this copy of { "solstone" } was installed. use the installer that installed it to update it.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e706c6174666f726d2e6f6c646572 = could not match this older build to a platform release. choose a platform version with { "--version" }.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e726f6c6c6261636b2e7061636b616765 = this package route cannot install an older journal. nothing was changed.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e7265706f72742e756e6b6e6f776e = update status is unknown.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726f6c6c6261636b2e65706f6368 = install the current version or another release in this upgrade epoch

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726f6c6c6261636b2e6c696e6b = current version link is missing or unreadable

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726f6c6c6261636b2e6d65746164617461 = installed release metadata is unreadable

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726f6c6c6261636b2e6e6f2e6f6c646572 = no eligible older build is retained. choose a platform version with { "--version" }, or reinstall the current release.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726f6c6c6261636b2e706f6c696379 = the current release has no supported retention policy

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726f6c6c6261636b2e72616e67652e616273656e74 = the installed release has no declared reader range. nothing was changed.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726f6c6c6261636b2e72616e67652e6f757473696465 = the selected version is outside the declared reader range. nothing was changed.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726f6c6c6261636b2e72657461696e6564 = that build is not retained; reinstall the current version

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726f6c6c6261636b2e7374617465 = the installer cannot establish the state of your journal. nothing was changed.

m6a6f75726e616c2e636c692e736f6c2e7570646174652e726f6c6c6261636b2e76657273696f6e = target version { $target_version } is not older than { $installed_version }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e73756d6d6172792e6661696c757265 = { $code }: { $message }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e73756d6d6172792e686f6c642e68656c64 = hold: held

m6a6f75726e616c2e636c692e736f6c2e7570646174652e73756d6d6172792e686f6c642e756e68656c64 = hold: unheld

m6a6f75726e616c2e636c692e736f6c2e7570646174652e73756d6d6172792e686f6c642e756e6b6e6f776e = hold: unknown

m6a6f75726e616c2e636c692e736f6c2e7570646174652e73756d6d6172792e6a6f75726e616c = journal: { $value }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e73756d6d6172792e6c616e65 = lane: { $value }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e73756d6d6172792e706c6174666f726d = platform: { $value }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e73756d6d6172792e726f757465 = route: { $value }

m6a6f75726e616c2e636c692e736f6c2e7570646174652e73756d6d6172792e746172676574 = available: { $available } (journal { $journal })

m6a6f75726e616c2e636c692e736f6c2e7573616765 = Usage: { "solstone <command> [args...]" }

m6a6f75726e616c2e636c692e73746174732e617267756d656e7473 = usage: { "solstone journal journal-stats [-h] [--no-cache] [-v] [-d]" }{ "\u000a" }{ "solstone journal journal-stats" }: error: unrecognized arguments: { $argument }

m6a6f75726e616c2e636c692e73746174732e63617573652e6661636574 = facet store error: { $detail }

m6a6f75726e616c2e636c692e73746174732e63617573652e696e76616c69642e646179 = invalid day { $day }; expected { "YYYYMMDD" } calendar date

m6a6f75726e616c2e636c692e73746174732e63617573652e696f = cannot access { $path }: { $detail }

m6a6f75726e616c2e636c692e73746174732e63617573652e70617468 = journal path error: { $detail }

m6a6f75726e616c2e636c692e73746174732e63617573652e72656164 = journal read error: { $detail }

m6a6f75726e616c2e636c692e73746174732e63617573652e736368656d61 = stats validation failed: { "schema_version" } is { $actual }, expected { $expected }

m6a6f75726e616c2e636c692e73746174732e63617573652e74696d657374616d70 = stats validation failed: { "generated_at" } must be non-empty

m6a6f75726e616c2e636c692e73746174732e63617573652e76616c69646174696f6e = stats validation failed: { $detail }

m6a6f75726e616c2e636c692e73746174732e63617573652e7772697465 = journal atomic-write error: { $detail }

m6a6f75726e616c2e636c692e73746174732e636f6e66696775726174696f6e = Error loading the journal's daily configuration: { $error }

m6a6f75726e616c2e636c692e73746174732e6461792e6361636865 = Day cache save failed for { $day }: { $error }

m6a6f75726e616c2e636c692e73746174732e656e756d6572617465 = Error enumerating journal days: { $error }

m6a6f75726e616c2e636c692e73746174732e68656c70 = usage: { "solstone journal journal-stats [-h] [--no-cache] [-v] [-d]" }{ "\u000a" }{ "\u000a" }Scan a { "solstone" } journal and generate statistics{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  " }{ "-h, --help" }{ "     " }show this help message and exit{ "\u000a" }{ "  " }{ "--no-cache" }{ "     " }Disable per-day caching (force re-scan all days){ "\u000a" }{ "  " }{ "-v, --verbose" }{ "  " }Enable verbose output{ "\u000a" }{ "  " }{ "-d, --debug" }{ "    " }Enable debug logging

m6a6f75726e616c2e636c692e73746174732e6f7665727269646573 = stats validation failed: failed to read talent overrides from { $path }

m6a6f75726e616c2e636c692e73746174732e7075626c69636174696f6e2e64757261626c65 = published; durability uncertain: { $source }

m6a6f75726e616c2e636c692e73746174732e7075626c69636174696f6e2e6f62736572766564 = published; observation uncertain: { $source }

m6a6f75726e616c2e636c692e73746174732e726f6f7473 = journal-stats failed: { $error }

m6a6f75726e616c2e636c692e73746174732e7361766564 = Statistics saved to { $path }

m6a6f75726e616c2e636c692e73746174732e746f6b656e732e6469726563746f7279 = Token directory read failed: { $path }

m6a6f75726e616c2e636c692e73746174732e746f6b656e732e6a736f6e = Invalid token { "JSON" } in { $path }: { $error }

m6a6f75726e616c2e636c692e73746174732e746f6b656e732e6c6f6164 = Token cache load failed for { $path }: { $error }

m6a6f75726e616c2e636c692e73746174732e746f6b656e732e72656164 = Token read failed for { $path }: { $error }

m6a6f75726e616c2e636c692e73746174732e746f6b656e732e73617665 = Token cache save failed for { $path }: { $error }

m6a6f75726e616c2e636c692e73746174732e76616c6964617465 = Error validating { "stats.json" }: { $error }

m6a6f75726e616c2e636c692e73746174732e7772697465 = Error writing { "stats.json" }: { $error }

m6a6f75726e616c2e636c692e73746174732e7772697474656e = { $i18nCategory ->
    [one] Wrote stats for { NUMBER($days, useGrouping: 0) } day to { $path }
   *[other] Wrote stats for { NUMBER($days, useGrouping: 0) } day(s) to { $path }
}

m6a6f75726e616c2e636c692e73747265616d732e617267756d656e7473 = usage: { "solstone journal streams [-h] [--rebuild] [-v] [-d] [name]" }{ "\u000a" }{ "solstone journal streams" }: error: unrecognized arguments: { $arguments }

m6a6f75726e616c2e636c692e73747265616d732e656d707479 = No streams found

m6a6f75726e616c2e636c692e73747265616d732e6865616465722e646179 = Last Day

m6a6f75726e616c2e636c692e73747265616d732e6865616465722e6e616d65 = Name

m6a6f75726e616c2e636c692e73747265616d732e6865616465722e7365676d656e74 = Last Segment

m6a6f75726e616c2e636c692e73747265616d732e6865616465722e73657175656e6365 = Seq

m6a6f75726e616c2e636c692e73747265616d732e6865616465722e74797065 = Type

m6a6f75726e616c2e636c692e73747265616d732e68656c70 = usage: { "solstone journal streams [-h] [--rebuild] [-v] [-d] [name]" }{ "\u000a\u000a" }Inspect and manage stream identity{ "\u000a\u000a" }positional arguments:{ "\u000a" }{ "  " }{ "name" }{ "           " }Stream name to inspect (omit to list all streams){ "\u000a\u000a" }options:{ "\u000a" }{ "  " }{ "-h, --help" }{ "     " }show this help message and exit{ "\u000a" }{ "  " }{ "--rebuild" }{ "      " }Reconstruct stream state from per-segment markers{ "\u000a" }{ "  " }{ "-v, --verbose" }{ "  " }Enable verbose output{ "\u000a" }{ "  " }{ "-d, --debug" }{ "    " }Enable debug logging

m6a6f75726e616c2e636c692e73747265616d732e696e73706563742e6d616c666f726d6564 = Could not read stream { $path }: malformed record

m6a6f75726e616c2e636c692e73747265616d732e696e73706563742e6e6f742e666f756e64 = Stream not found: { $name }

m6a6f75726e616c2e636c692e73747265616d732e6c6973742e6572726f72 = Could not list streams: { $error }

m6a6f75726e616c2e636c692e73747265616d732e6d61726b65722e73657175656e6365 = unreadable marker { $path }: invalid sequence

m6a6f75726e616c2e636c692e73747265616d732e6d61726b65722e756e7265616461626c65 = unreadable marker { $path }: could not read marker

m6a6f75726e616c2e636c692e73747265616d732e72656275696c642e656d707479 = { $i18nCategory ->
    [one] No streams found ({ $segments } segment scanned)
   *[other] No streams found ({ $segments } segments scanned)
}

m6a6f75726e616c2e636c692e73747265616d732e72656275696c642e726573756c74 = { $i18nCategory ->
    [one] Rebuilt { $streams } stream from { $i18nCategory-segment_quantity ->
        [one] { $segments } segment:
       *[other] { $segments } segments:
    }
   *[other] Rebuilt { $streams } stream(s) from { $i18nCategory-segment_quantity ->
        [one] { $segments } segment:
       *[other] { $segments } segments:
    }
}

m6a6f75726e616c2e636c692e73747265616d732e7265636f72642e6d616c666f726d6564 = Could not read stream record { $path }: malformed record

m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6168656164 = { $stream }: stream state sequence is ahead of markers; unchanged

m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6c6f636b6564 = { $stream }: could not lock { $path }; unchanged

m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6d616c666f726d6564 = { $stream }: malformed stream record at { $path }; unchanged

m6a6f75726e616c2e636c692e73747265616d732e7265706169722e6d697373696e67 = { $stream }: no stream record at { $path }; unchanged

m6a6f75726e616c2e636c692e73747265616d732e7265706169722e77726974652e6661696c6564 = { $stream }: write failed for { $path }: atomic publication failed

m6a6f75726e616c2e636c692e73747265616d732e7365676d656e74732e6572726f72 = Could not list segments: { $error }

m6a6f75726e616c2e636c692e73747265616d732e73757065727669736f72 = journal isn't running. start it with '{ "solstone journal up" }' and retry.

m6a6f75726e616c2e636c692e74616c656e742e636f6d706f73652e66616365742e6d697373696e67 = facet '{ $facet }' not found

m6a6f75726e616c2e636c692e74616c656e742e636f6d706f73652e70726f6d70742e706174682e6861732e6e6f2e706172656e74 = talent { $talent }: prompt path has no parent

m6a6f75726e616c2e636c692e74616c656e742e636f6d706f73652e70726f6d70742e706174682e69732e6d697373696e67 = talent { $talent }: prompt path is missing

m6a6f75726e616c2e636c692e74616c656e742e636f6d706f73652e736368656d612e6469726563746f72792e69732e756e617661696c61626c65 = talent { $talent }: schema directory is unavailable: { $error }

m6a6f75726e616c2e636c692e74616c656e742e636f6d706f73652e736368656d612e66696c652e636f756c642e6e6f742e62652e72656164 = talent { $talent }: schema file could not be read: { $error }

m6a6f75726e616c2e636c692e74616c656e742e636f6d706f73652e736368656d612e66696c652e69732e6e6f742e612e76616c69642e6a736f6e2e736368656d61 = talent { $talent }: schema file is not a valid { "JSON Schema" }: { $path }

m6a6f75726e616c2e636c692e74616c656e742e636f6d706f73652e736368656d612e66696c652e69732e6e6f742e76616c69642e6a736f6e = talent { $talent }: schema file is not valid { "JSON" }: { $path }

m6a6f75726e616c2e636c692e74616c656e742e636f6d706f73652e736368656d612e66696c652e6e6f742e666f756e642e6361757365 = talent { $talent }: schema file not found: { $error }

m6a6f75726e616c2e636c692e74616c656e742e636f6d706f73652e736368656d612e66696c652e6e6f742e666f756e642e70617468 = talent { $talent }: schema file not found: { $path }

m6a6f75726e616c2e636c692e74616c656e742e636f6d706f73652e736368656d612e6d7573742e62652e612e737472696e672e676f74 = talent { $talent }: schema must be a string, got { $type_name }: { $value }

m6a6f75726e616c2e636c692e74616c656e742e636f6d706f73652e736368656d612e706174682e657363617065732e74616c656e742e6469726563746f7279 = talent { $talent }: schema path escapes talent directory: { $path }

m6a6f75726e616c2e636c692e74616c656e742e636f6d706f73652e736368656d612e706174682e6d7573742e62652e72656c6174697665 = talent { $talent }: schema path must be relative: { $path }

m6a6f75726e616c2e636c692e74616c656e742e636f6d706f73652e736368656d612e706174682e6d7573742e6e6f742e636f6e7461696e = talent { $talent }: schema path must not contain '..': { $path }

m6a6f75726e616c2e636c692e74616c656e742e636f6d706f73652e73657474696e6773 = failed to read journal config: { $detail }

m6a6f75726e616c2e636c692e74616c656e742e636f6e6669672e6163636573732e72657469726564 = Prompt '{ $talent }' sets retired { "'access_tier'" }

m6a6f75726e616c2e636c692e74616c656e742e636f6e6669672e61637469766974696573 = Activity-scheduled prompt '{ $talent }' must have a non-empty { "'activities'" } list (activity types to match, or { "[\"*\"]" } for all types).

m6a6f75726e616c2e636c692e74616c656e742e636f6e6669672e6275646765742e696e76616c6964 = Prompt '{ $talent }' has invalid { "'max_output_tokens'" } { $value }. Expected a positive integer measured against the bundled model.

m6a6f75726e616c2e636c692e74616c656e742e636f6e6669672e6275646765742e7265717569726564 = Prompt '{ $talent }' is missing required { "'max_output_tokens'" }. Measure its largest output on the bundled model and declare 1.5 times that, rounded up to a multiple of 256.

m6a6f75726e616c2e636c692e74616c656e742e636f6e6669672e6275646765742e72657469726564 = Prompt '{ $talent }' declares retired { "'thinking_budget'" }. Thinking is set only in Thinking, for the owner's own model.

m6a6f75726e616c2e636c692e74616c656e742e636f6e6669672e6377642e72657469726564 = Prompt '{ $talent }' sets retired { "'cwd'" }

m6a6f75726e616c2e636c692e74616c656e742e636f6e6669672e66726f6e746d6174746572 = failed to parse frontmatter from { $path }

m6a6f75726e616c2e636c692e74616c656e742e636f6e6669672e6f75747075742e7265717569726564 = Prompt '{ $talent }' has type={ "'generate'" } but is missing required { "'output'" } field.

m6a6f75726e616c2e636c692e74616c656e742e636f6e6669672e7072696f72697479 = Scheduled prompt '{ $talent }' is missing required { "'priority'" } field. All prompts with { "'schedule'" } must declare an explicit priority.

m6a6f75726e616c2e636c692e74616c656e742e636f6e6669672e72656164 = failed to read { $path }: { $error }

m6a6f75726e616c2e636c692e74616c656e742e636f6e6669672e73746174 = failed to stat { $path }: { $error }

m6a6f75726e616c2e636c692e74616c656e742e636f6e6669672e7374656d = talent filename has no { "UTF-8" } stem: { $path }

m6a6f75726e616c2e636c692e74616c656e742e636f6e6669672e747970652e696e76616c6964 = Prompt '{ $talent }' has invalid type { $value }. Expected { "'generate'" }.

m6a6f75726e616c2e636c692e74616c656e742e636f6e6669672e747970652e7265717569726564 = Prompt '{ $talent }' has output but is missing required { "'type'" } field.

m6a6f75726e616c2e636c692e74616c656e742e6572726f722e6c6973742e63686f696365 = usage: { "solstone journal talent list [-h] [--schedule {daily,segment,activity}]" }{ "\u000a" }{ "solstone journal talent list" }: error: argument { $flag }: invalid choice: '{ $value }' (choose from { $choices })

m6a6f75726e616c2e636c692e74616c656e742e6572726f722e6c6973742e756e6b6e6f776e = usage: { "solstone journal talent list [-h] [--schedule {daily,segment,activity}]" }{ "\u000a" }{ "solstone journal talent list" }: error: unrecognized arguments: { $arguments }

m6a6f75726e616c2e636c692e74616c656e742e6572726f722e6c6973742e76616c7565 = usage: { "solstone journal talent list [-h] [--schedule {daily,segment,activity}]" }{ "\u000a" }{ "solstone journal talent list" }: error: argument { $flag }: expected one argument

m6a6f75726e616c2e636c692e74616c656e742e6572726f722e6c6f672e7265717569726564 = usage: { "solstone journal talent log [-h] [--json] [--full] id" }{ "\u000a" }{ "solstone journal talent log" }: error: the following arguments are required: { $arguments }

m6a6f75726e616c2e636c692e74616c656e742e6572726f722e6c6f67732e636f756e74 = usage: { "solstone journal talent logs [-h] [-c COUNT] [--day YYYYMMDD] [--daily] [--errors]" }{ "\u000a" }{ "solstone journal talent logs" }: error: argument { "-c/--count" }: invalid int value: '{ $value }'

m6a6f75726e616c2e636c692e74616c656e742e6572726f722e6c6f67732e76616c7565 = usage: { "solstone journal talent logs [-h] [-c COUNT] [--day YYYYMMDD] [--daily] [--errors]" }{ "\u000a" }{ "solstone journal talent logs" }: error: argument { $flag }: expected one argument

m6a6f75726e616c2e636c692e74616c656e742e6572726f722e70617373616765732e7265717569726564 = usage: { "solstone journal talent passages [-h] [--json] --day YYYYMMDD" }{ "\u000a" }{ "solstone journal talent passages" }: error: the following arguments are required: { $arguments }

m6a6f75726e616c2e636c692e74616c656e742e6572726f722e70617373616765732e756e6b6e6f776e = usage: { "solstone journal talent passages [-h] [--json] --day YYYYMMDD" }{ "\u000a" }{ "solstone journal talent passages" }: error: unrecognized arguments: { $arguments }

m6a6f75726e616c2e636c692e74616c656e742e6572726f722e70617373616765732e76616c7565 = usage: { "solstone journal talent passages [-h] [--json] --day YYYYMMDD" }{ "\u000a" }{ "solstone journal talent passages" }: error: argument { $flag }: expected one argument

m6a6f75726e616c2e636c692e74616c656e742e6572726f722e726f6f742e63686f696365 = usage: { "solstone journal talent [-h] [-v] [-d] {list,show,passages,logs,log} ..." }{ "\u000a" }{ "solstone journal talent" }: error: argument { "subcommand" }: invalid choice: '{ $value }' (choose from { "'list', 'show', 'passages', 'logs', 'log'" })

m6a6f75726e616c2e636c692e74616c656e742e6572726f722e726f6f742e656e636f64696e67 = usage: { "solstone journal talent [-h] [-v] [-d] {list,show,passages,logs,log} ..." }{ "\u000a" }{ "solstone journal talent" }: error: arguments are not valid { "UTF-8" }

m6a6f75726e616c2e636c692e74616c656e742e6572726f722e726f6f742e756e6b6e6f776e = usage: { "solstone journal talent [-h] [-v] [-d] {list,show,passages,logs,log} ..." }{ "\u000a" }{ "solstone journal talent" }: error: unrecognized arguments: { $arguments }

m6a6f75726e616c2e636c692e74616c656e742e6572726f722e73686f772e7265717569726564 = usage: { "solstone journal talent show [-h] [--json] [--prompt] [--day YYYYMMDD]" }{ "\u000a" }{ "solstone journal talent show" }: error: the following arguments are required: { $arguments }

m6a6f75726e616c2e636c692e74616c656e742e6572726f722e73686f772e756e6b6e6f776e = usage: { "solstone journal talent show [-h] [--json] [--prompt] [--day YYYYMMDD]" }{ "\u000a" }{ "solstone journal talent show" }: error: unrecognized arguments: { $arguments }

m6a6f75726e616c2e636c692e74616c656e742e6572726f722e73686f772e76616c7565 = usage: { "solstone journal talent show [-h] [--json] [--prompt] [--day YYYYMMDD]" }{ "\u000a" }{ "solstone journal talent show" }: error: argument { $flag }: expected one argument

m6a6f75726e616c2e636c692e74616c656e742e66616365742e6d616c666f726d6564 = malformed data in { $path }

m6a6f75726e616c2e636c692e74616c656e742e66616365742e6d616c666f726d65642e6c696e65 = malformed data in { $path } at line { $line }

m6a6f75726e616c2e636c692e74616c656e742e66616365742e6f626a656374 = facet declaration is not an object: { $path }

m6a6f75726e616c2e636c692e74616c656e742e666c6f6f722e696e76616c69642e627564676574 = missing or invalid { "max_output_tokens" }

m6a6f75726e616c2e636c692e74616c656e742e666c6f6f722e6d69736d61746368 = { $i18nCategory ->
    [one] entry count mismatch: parsed { NUMBER($entries, useGrouping: 0) } entry but have { $i18nCategory-sources ->
        [one] { NUMBER($sources, useGrouping: 0) } entry source
       *[other] { NUMBER($sources, useGrouping: 0) } entry sources
    }
   *[other] entry count mismatch: parsed { NUMBER($entries, useGrouping: 0) } entries but have { $i18nCategory-sources ->
        [one] { NUMBER($sources, useGrouping: 0) } entry source
       *[other] { NUMBER($sources, useGrouping: 0) } entry sources
    }
}

m6a6f75726e616c2e636c692e74616c656e742e68656c702e6c697374 = usage: { "solstone journal talent list [-h] [--schedule {daily,segment,activity}]" }{ "\u000a" }{ "                           [--source {system,app}] [--disabled] [--json]" }{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  " }{ "-h, --help" }{ "            " }show this help message and exit{ "\u000a" }{ "  --schedule {daily,segment,activity}" }{ "\u000a" }                        Filter by schedule type{ "\u000a" }{ "  --source {system,app}" }{ "\u000a" }                        Filter by origin{ "\u000a" }{ "  " }{ "--disabled" }{ "            " }Include disabled prompts{ "\u000a" }{ "  " }{ "--json" }{ "                " }Output as { "JSONL" }

m6a6f75726e616c2e636c692e74616c656e742e68656c702e6c6f67 = usage: { "solstone journal talent log [-h] [--json] [--full] id" }{ "\u000a" }{ "\u000a" }positional arguments:{ "\u000a" }  { "id          " }Agent ID{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  " }{ "-h, --help" }{ "  " }show this help message and exit{ "\u000a" }{ "  " }{ "--json" }{ "      " }Output raw { "JSONL" }{ "\u000a" }{ "  " }{ "--full" }{ "      " }Expand event details

m6a6f75726e616c2e636c692e74616c656e742e68656c702e6c6f6773 = usage: { "solstone journal talent logs [-h] [-c COUNT] [--day YYYYMMDD] [--daily] [--errors]" }{ "\u000a" }{ "                           [--summary]" }{ "\u000a" }{ "                           [agent]" }{ "\u000a" }{ "\u000a" }positional arguments:{ "\u000a" }  { "agent                 " }Filter to a specific agent{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  " }{ "-h, --help" }{ "            " }show this help message and exit{ "\u000a" }{ "  -c COUNT, --count COUNT" }{ "\u000a" }                        Number of runs to show (default: 20){ "\u000a" }{ "  " }{ "--day YYYYMMDD" }{ "        " }Show only runs from this day{ "\u000a" }{ "  " }{ "--daily" }{ "               " }Show only daily-scheduled runs{ "\u000a" }{ "  " }{ "--errors" }{ "              " }Show only error runs{ "\u000a" }{ "  " }{ "--summary" }{ "             " }Show grouped summary

m6a6f75726e616c2e636c692e74616c656e742e68656c702e7061737361676573 = usage: { "solstone journal talent passages [-h] [--json] --day YYYYMMDD" }{ "\u000a" }{ "                           --facet NAME --activity ID" }{ "\u000a" }                           name{ "\u000a" }{ "\u000a" }Copy exact passages from the source an activity talent reads.{ "\u000a" }A short source unit is kept whole without a model call; a longer one{ "\u000a" }takes one completion to choose what to copy. No passages are written to{ "\u000a" }the journal; a completion's usage is logged like any other.{ "\u000a" }{ "\u000a" }positional arguments:{ "\u000a" }  { "name                  " }Activity talent name, such as work or conversation{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  " }{ "-h, --help" }{ "            " }show this help message and exit{ "\u000a" }{ "  " }{ "--json" }{ "                " }Output the passage document as { "JSON" }{ "\u000a" }{ "  " }{ "--day YYYYMMDD" }{ "        " }Day of the activity{ "\u000a" }{ "  " }{ "--facet NAME" }{ "          " }Facet of the activity{ "\u000a" }{ "  " }{ "--activity ID" }{ "         " }Activity ID

m6a6f75726e616c2e636c692e74616c656e742e68656c702e726f6f74 = usage: { "solstone journal talent [-h] [-v] [-d] {list,show,passages,logs,log} ..." }{ "\u000a" }{ "\u000a" }Inspect talent prompt configurations{ "\u000a" }{ "\u000a" }positional arguments:{ "\u000a" }{ "  {list,show,passages,logs,log}" }{ "\u000a" }    { "list                " }List prompts grouped by schedule{ "\u000a" }    { "show                " }Show details for a specific prompt{ "\u000a" }    { "passages            " }Copy source passages from an activity's source{ "\u000a" }    { "logs                " }Show recent talent run log{ "\u000a" }    { "log                 " }Show events for an agent run{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  " }{ "-h, --help" }{ "            " }show this help message and exit{ "\u000a" }{ "  " }{ "-v, --verbose" }{ "         " }Enable verbose output{ "\u000a" }{ "  " }{ "-d, --debug" }{ "           " }Enable debug logging

m6a6f75726e616c2e636c692e74616c656e742e68656c702e73686f77 = usage: { "solstone journal talent show [-h] [--json] [--prompt] [--day YYYYMMDD]" }{ "\u000a" }{ "                           [--segment HHMMSS_LEN] [--facet NAME]" }{ "\u000a" }{ "                           [--activity ID] [--query TEXT] [--full]" }{ "\u000a" }                           name{ "\u000a" }{ "\u000a" }positional arguments:{ "\u000a" }  { "name                  " }Prompt name{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  " }{ "-h, --help" }{ "            " }show this help message and exit{ "\u000a" }{ "  " }{ "--json" }{ "                " }Output as { "JSONL" }{ "\u000a" }{ "  " }{ "--prompt" }{ "              " }Show full prompt context (dry-run mode){ "\u000a" }{ "  " }{ "--day YYYYMMDD" }{ "        " }Day for prompt context{ "\u000a" }{ "  " }{ "--segment HHMMSS_LEN" }{ "  " }Segment for segment-scheduled prompts{ "\u000a" }{ "  " }{ "--facet NAME" }{ "          " }Facet for multi-facet prompts{ "\u000a" }{ "  " }{ "--activity ID" }{ "         " }Activity ID for activity-scheduled prompts{ "\u000a" }{ "  " }{ "--query TEXT" }{ "          " }Sample query for tool agents{ "\u000a" }{ "  " }{ "--full" }{ "                " }Show full content without truncation

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e6272696566696e672e646174652e6f766572666c6f77 = briefing date overflow

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e6272696566696e672e6661696c6564 = morning briefing pre-hook failed: { $error }

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e627564676574 = prepared request does not fit the floor input budget

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e6461696c792e6c6f6f6b6261636b = daily schedule lookback outside { "1..=366" }

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e6461792e696e76616c6964 = invalid or unparseable day: '{ $day }'

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e6461792e6d697373696e67 = missing day

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e646f63756d656e742e73747265616d = not a document import segment

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e656e746974792e6d696e696d756d = entity packet for { $entity } is at least { $count } characters; maximum is { $maximum }

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e656e746974792e6e616d65 = missing entity name

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e656e746974792e73697a65 = entity packet for { $entity } is { $count } characters; maximum is { $maximum }

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e66616365742e6d697373696e67 = missing facet

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e66616365742e72656164 = facet { $facet } could not be read: { $error }

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e6661636574732e6c697374 = facets could not be listed: { $error }

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e66616374732e73697a65 = entity facts context is { $count } characters; maximum is { $maximum }

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e6661696c757265 = { $phase } hook '{ $stage }' for talent '{ $talent }': { $detail }

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e6964656e7469746965732e72656164 = entity identities could not be read: { $error }

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e6964656e746974792e6f626a656374 = entity identity is not an object: { $path }

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e6c6f6f6b75702e636f6e74726f6c = invalid path component '{ $candidate }': the name contains a control character

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e6c6f6f6b75702e646f74636f6d706f6e656e74 = invalid path component '{ $candidate }': { "'.'" } and { "'..'" } are not valid names

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e6c6f6f6b75702e656d707479 = invalid path component '{ $candidate }': the name is empty

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e6c6f6f6b75702e6b696e64 = { $path } is not a directory ({ $kind })

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e6c6f6f6b75702e6c61796f7574 = direct layout requires stream "{ "_default" }", got { $stream }

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e6c6f6f6b75702e726f6f746f72707265666978 = invalid path component '{ $candidate }': the name must be a single relative component

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e6c6f6f6b75702e736570617261746f72 = invalid path component '{ $candidate }': the name contains a path separator

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e706f737369626c652e6e6f2e70726f6d7074 = the talent has no prompt

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e7265736f6c7574696f6e2e616273656e74 = resolved ambiguity { $ambiguity } names entity { $entity } absent from this resolution call

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e7265736f6c7574696f6e2e68696464656e = resolved ambiguity { $ambiguity } names hidden entity { $entity }

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e7265766965772e6368616e676564 = review inputs changed during preparation

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e73747265616d2e6c61796f7574 = direct layout requires stream "{ "_default" }", got "{ $stream }"

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e737567676573742e73697a65 = entity suggest context is { $count } characters; maximum is { $maximum }

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e73756767657374696f6e732e6172726179 = suggestions JSON missing { "entities" } array

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e73756767657374696f6e732e6a736f6e = invalid suggestions JSON in { $path }: { $error }

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e73756767657374696f6e732e6d697373696e67 = missing suggestions artifact: { $path }

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e73756e646179 = request day { $day } is not a Sunday

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e7472616e7363726970742e6279746573 = transcript bytes are not valid UTF-8

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e7472616e7363726970742e72656164 = transcript { $file } could not be read: { $error }

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e7472616e7363726970742e75746638 = transcript { $file } is not UTF-8: { $error }

m6a6f75726e616c2e636c692e74616c656e742e686f6f6b2e7765656b2e696e636f6d706c657465 = week ending { $end } has not completed before today { $today }

m6a6f75726e616c2e636c692e74616c656e742e6c6173742e616765 = { $age } ago

m6a6f75726e616c2e636c692e74616c656e742e6c6173742e6167652e6475726174696f6e = { $age } ago ({ $duration })

m6a6f75726e616c2e636c692e74616c656e742e6c6973742e656d707479 = No prompts found matching filters.

m6a6f75726e616c2e636c692e74616c656e742e6c6973742e67726f75702e6163746976697479 = activity:

m6a6f75726e616c2e636c692e74616c656e742e6c6973742e67726f75702e636164656e6365 = cadence:

m6a6f75726e616c2e636c692e74616c656e742e6c6973742e67726f75702e6461696c79 = daily:

m6a6f75726e616c2e636c692e74616c656e742e6c6973742e67726f75702e7365676d656e74 = segment:

m6a6f75726e616c2e636c692e74616c656e742e6c6973742e67726f75702e756e7363686564756c6564 = unscheduled:

m6a6f75726e616c2e636c692e74616c656e742e6c6973742e67726f75702e7765656b6c79 = weekly:

m6a6f75726e616c2e636c692e74616c656e742e6c6973742e6865616465722e6c617374 = LAST RUN

m6a6f75726e616c2e636c692e74616c656e742e6c6973742e6865616465722e6e616d65 = NAME

m6a6f75726e616c2e636c692e74616c656e742e6c6973742e6865616465722e74616773 = TAGS

m6a6f75726e616c2e636c692e74616c656e742e6c6973742e6865616465722e7469746c65 = TITLE

m6a6f75726e616c2e636c692e74616c656e742e6c6973742e7461672e64697361626c6564 = disabled

m6a6f75726e616c2e636c692e74616c656e742e6c6973742e7461672e6661696c = FAIL

m6a6f75726e616c2e636c692e74616c656e742e6c6973742e76697369626c65 = { $i18nCategory ->
    [one] { NUMBER($count, useGrouping: 0) } prompt ({ $hidden } disabled hidden, use { "--disabled" })
   *[other] { NUMBER($count, useGrouping: 0) } prompts ({ $hidden } disabled hidden, use { "--disabled" })
}

m6a6f75726e616c2e636c692e74616c656e742e6c6f672e6576656e742e7468696e6b696e67 = think

m6a6f75726e616c2e636c692e74616c656e742e6c6f672e6576656e742e746f6f6c = tool

m6a6f75726e616c2e636c692e74616c656e742e6c6f672e6576656e742e75706461746564 = updated

m6a6f75726e616c2e636c692e74616c656e742e6c6f672e6d697373696e67 = Talent run not found: { $run }

m6a6f75726e616c2e636c692e74616c656e742e6c6f672e746f6b656e73 = { $result } [{ $input }in/{ $output }out]

m6a6f75726e616c2e636c692e74616c656e742e6c6f67732e64617465 = { $month } { $day_time }

m6a6f75726e616c2e636c692e74616c656e742e6c6f67732e6461792e696e76616c6964 = Invalid { "--day" } format: { $day }. Expected { "YYYYMMDD" }.

m6a6f75726e616c2e636c692e74616c656e742e6c6f67732e746f74616c = total

m6a6f75726e616c2e636c692e74616c656e742e70617373616765732e67656e6572617465 = source passages could not reach generate: { $error }

m6a6f75726e616c2e636c692e74616c656e742e70617373616765732e67656e65726174652e6d697373696e67 = source passages could not reach generate: missing sibling executable { $path }

m6a6f75726e616c2e636c692e74616c656e742e70617373616765732e67656e65726174652e706172656e74 = source passages could not reach generate: current executable has no parent

m6a6f75726e616c2e636c692e74616c656e742e70617373616765732e696e636f6d706c657465 = some source units have no passages; see each unit's outcome

m6a6f75726e616c2e636c692e74616c656e742e70617373616765732e756e617661696c61626c65 = source passages unavailable: { $detail }

m6a6f75726e616c2e636c692e74616c656e742e70617373616765732e756e69742e6661696c6564 = # unit { $index } · { $kind } · no passages: { $reason }

m6a6f75726e616c2e636c692e74616c656e742e70617373616765732e756e69742e6661696c65642e74696d65 = # unit { $index } · { $kind } · { $time } · no passages: { $reason }

m6a6f75726e616c2e636c692e74616c656e742e70617373616765732e756e69742e6e6f742e617474656d70746564 = # unit { $index } · { $kind } · not attempted

m6a6f75726e616c2e636c692e74616c656e742e70617373616765732e756e69742e6e6f742e617474656d707465642e74696d65 = # unit { $index } · { $kind } · { $time } · not attempted

m6a6f75726e616c2e636c692e74616c656e742e70617373616765732e756e69742e7061737361676573 = { $i18nCategory ->
    [one] # unit { $index } · { $kind } · { NUMBER($count, useGrouping: 0) } passage
   *[other] # unit { $index } · { $kind } · { NUMBER($count, useGrouping: 0) } passage(s)
}

m6a6f75726e616c2e636c692e74616c656e742e70617373616765732e756e69742e70617373616765732e74696d65 = { $i18nCategory ->
    [one] # unit { $index } · { $kind } · { $time } · { NUMBER($count, useGrouping: 0) } passage
   *[other] # unit { $index } · { $kind } · { $time } · { NUMBER($count, useGrouping: 0) } passage(s)
}

m6a6f75726e616c2e636c692e74616c656e742e70617373616765732e766572626174696d = Verbatim from source:

m6a6f75726e616c2e636c692e74616c656e742e706174682e6162736f6c757465 = journal path must be relative

m6a6f75726e616c2e636c692e74616c656e742e706174682e6261636b736c617368 = journal path must use forward slashes

m6a6f75726e616c2e636c692e74616c656e742e706174682e636f6d706f6e656e74 = journal path contains invalid component

m6a6f75726e616c2e636c692e74616c656e742e706174682e646179 = day must be { "YYYYMMDD" }

m6a6f75726e616c2e636c692e74616c656e742e706174682e656d707479 = journal path must not be empty

m6a6f75726e616c2e636c692e74616c656e742e706174682e657363617065 = { $relative } escapes the journal root (resolved to { $path })

m6a6f75726e616c2e636c692e74616c656e742e706174682e756e7265616461626c65 = journal path is not readable

m6a6f75726e616c2e636c692e74616c656e742e707265706172652e636f6e66696775726174696f6e2e6e616d652e6465636c617265732e616e2e696e76616c69642e6d6f62696c652e6c6f63616c2e74696d656f7574 = configuration '{ $name }' declares an invalid { "mobile_local_timeout_s" }

m6a6f75726e616c2e636c692e74616c656e742e707265706172652e696e76616c69642e636f6e6669672e6d7573742e686176652e747970652e6461792e6f722e70726f6d7074 = Invalid config: must have { "'type'" }, { "'day'" }, or { "'prompt'" }

m6a6f75726e616c2e636c692e74616c656e742e707265706172652e696e76616c69642e636f6e6669672e7365676d656e742e6f722e7370616e2e72657175697265732e646179 = Invalid config: { "'segment'" } or { "'span'" } requires { "'day'" }

m6a6f75726e616c2e636c692e74616c656e742e707265706172652e696e76616c69642e74616c656e742e747970652e6b696e64 = invalid talent type '{ $kind }'

m6a6f75726e616c2e636c692e74616c656e742e707265706172652e6d656d6f72792e696e636f6d706c657465 = private memory context is incomplete

m6a6f75726e616c2e636c692e74616c656e742e707265706172652e726571756573742e6669656c642e6669656c642e69732e72657469726564 = request field '{ $field }' is retired

m6a6f75726e616c2e636c692e74616c656e742e707265706172652e726571756573742e6f76657272696465732e6669656c642e666f722e74616c656e742e6e616d652e6172652e6e6f742e616c6c6f776564 = Request overrides '{ $field }' for talent '{ $name }' are not allowed ({ $declared } != { $requested })

m6a6f75726e616c2e636c692e74616c656e742e707265706172652e726571756573742e6f76657272696465732e666f722e6669656c642e6172652e6e6f742e616c6c6f776564 = request overrides for '{ $field }' are not allowed; thinking provider and model are configured only in Thinking

m6a6f75726e616c2e636c692e74616c656e742e707265706172652e726571756573742e6f76657272696465732e666f722e6669656c642e6172652e6e6f742e616c6c6f7765642e627564676574 = request overrides for '{ $field }' are not allowed; a talent's budgets are its own

m6a6f75726e616c2e636c692e74616c656e742e707265706172652e7365676d656e742e6d697373696e67 = { $i18nCategory ->
    [one] Segment directory not found: { $segments }
   *[other] Segment directories not found: { $segments }
}

m6a6f75726e616c2e636c692e74616c656e742e707265706172652e74616c656e742e6e616d652e6465636c617265732e6669656c642e696e2e66726f6e746d6174746572 = talent '{ $name }' declares '{ $field }' in frontmatter; thinking provider and model are configured only in Thinking

m6a6f75726e616c2e636c692e74616c656e742e707265706172652e74616c656e742e6e616d652e6465636c617265732e6f7574626f756e642e617070726f76616c2e696e2e66726f6e746d6174746572 = talent '{ $name }' declares { "'outbound_approval'" } in frontmatter; this field is launch-config-only and may not come from a talent definition

m6a6f75726e616c2e636c692e74616c656e742e707265706172652e74616c656e742e6e616d652e6e6f742e666f756e64 = talent '{ $name }' not found

m6a6f75726e616c2e636c692e74616c656e742e707265706172652e74616c656e742e726571756573742e6d697373696e672e6e616d65 = talent request missing name

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e616c6c2e6c696e6573 = (use { "--full" } to see all { $count } lines)

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e62616e6e6572 = Preview of what this talent would send. Same assembly a real run uses; the model is not called.

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e656666656374697665 = Effective prompt for: { $talent }

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e656d707479 = (empty)

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e696e737472756374696f6e = INSTRUCTION

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e6e6f2e646179 = No day given, so this preview has no day's recordings in it. Pass { "--day" } { "YYYYMMDD" } to include them.

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e6f6d6974746564 = { $i18nCategory ->
    [one] ... ({ NUMBER($count, useGrouping: 0) } line omitted)
   *[other] ... ({ NUMBER($count, useGrouping: 0) } lines omitted)
}

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e6f6d69747465642e6c6567616379 = ... ({ $count } lines omitted)

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e70726573746570 = Prompt preview cannot assemble this talent: a required pre-step is not available.

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e63686f6f73652e612e636f6d706c657465642e6f776e65722e61637469766974792e6e6f742e616e2e616e7469636970617465642e6f722e636f6769 = Choose a completed owner activity, not an anticipated or cogitate record.

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e63686f6f73652e612e74616c656e742e636f6e666967757265642e666f722e746869732e61637469766974792e6b696e64 = Choose a talent configured for this activity kind.

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e63686f6f73652e612e74616c656e742e746861742e6c6f6164732e6974732e61637469766974792e732e736f7572636573 = Choose a talent that loads its activity's sources.

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e63686f6f73652e612e74616c656e742e746861742e72656164732e6974732e61637469766974792e736f757263652e776974686f75742e612e707265 = Choose a talent that reads its activity source without a pre-step.

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e63686f6f73652e612e74616c656e742e77686f73652e72657175697265642e7072652e737465702e69732e617661696c61626c652e696e2e746865 = Choose a talent whose required pre-step is available in the native runtime.

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e63686f6f73652e612e74616c656e742e77686f73652e7363686564756c652e69732e6163746976697479 = Choose a talent whose schedule is { "activity." }

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e63686f6f73652e616e2e61637469766974792e69642e73746f7265642e696e2e7468652e7265717565737465642e6461792e616e642e6661636574 = Choose an activity ID stored in the requested day and facet.

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e63686f6f73652e616e2e61637469766974792e776974682e61742e6c656173742e6f6e652e7265636f726465642e7365676d656e74 = Choose an activity with at least one recorded segment.

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e63686f6f73652e6569746865722e61637469766974792e6f722e7365676d656e742e6e6f742e626f7468 = Choose either { "--activity" } or { "--segment" }, not both.

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e636f72726563742e7468652e61637469766974792e707265766965772e696e7075742e7468656e2e72657472792e64657461696c = Correct the activity preview input, then retry: { $detail }

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e636f72726563742e7468652e74616c656e742e636f6e66696775726174696f6e2e64657461696c = Correct the talent configuration: { $detail }

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e6d616b652e7468652e73656c65637465642e61637469766974792e616e642e74616c656e742e656c696769626c652e7468656e2e7265747279 = Make the selected activity and talent eligible, then retry.

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e6d616b652e7468652e73656c65637465642e74616c656e742e656c696769626c652e7468656e2e7265747279 = Make the selected talent eligible, then retry.

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e706173732e6461792e797979796d6d64642e66616365742e6e616d652e616e642e61637469766974792e6964 = Pass { "--day" } { "YYYYMMDD" }, { "--facet" } { "NAME" } and { "--activity" } ID.

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e706173732e6461792e797979796d6d64642e776974682e6163746976697479 = Pass { "--day" } { "YYYYMMDD" } with { "--activity" }.

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e706173732e66616365742e6e616d652e776974682e6163746976697479 = Pass { "--facet" } { "NAME" } with { "--activity" }.

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e72656d6f76652e6475706c69636174652e7265636f7264732e776974682e746869732e69642e696e2e7468652e7265717565737465642e6461792e61 = Remove duplicate records with this ID in the requested day and facet.

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e7265706169722e7468652e61637469766974792e7370616e2e736f2e65766572792e7365676d656e742e69732e612e6e6f6e2e656d7074792e737472 = Repair the activity span so every segment is a non-empty string.

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e726573746f72652e612e7265616461626c652e61637469766974792e7265636f72642e66696c652e7468656e2e72657472792e64657461696c = Restore a readable activity record file, then retry: { $detail }

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e726573746f72652e612e7265616461626c652e7365676d656e742e6469726563746f72792e7468656e2e72657472792e64657461696c = Restore a readable segment directory, then retry: { $detail }

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e726573746f72652e7265616461626c652e7365676d656e742e6469726563746f726965732e7468656e2e72657472792e64657461696c = Restore readable segment directories, then retry: { $detail }

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e7265636f766572792e726573746f72652e7468652e73656c65637465642e61637469766974792e7365676d656e742e7468656e2e7265747279 = Restore the selected activity segment, then retry.

m6a6f75726e616c2e636c692e74616c656e742e707265766965772e736b6970706564 = This talent would not run: { $reason }

m6a6f75726e616c2e636c692e74616c656e742e73686f772e626f6479 = body:

m6a6f75726e616c2e636c692e74616c656e742e73686f772e626f64792e6c696e6573 = { $i18nCategory ->
    [one] { NUMBER($count, useGrouping: 0) } line
   *[other] { NUMBER($count, useGrouping: 0) } lines
}

m6a6f75726e616c2e636c692e74616c656e742e73686f772e686f6f6b = Prompt '{ $talent }' is a hook prompt and cannot be run directly.

m6a6f75726e616c2e636c692e74616c656e742e73686f772e6d697373696e67 = Prompt not found: { $talent }

m6a6f75726e616c2e636c692e74616c656e742e73686f772e6d697373696e672e70617468 = Prompt not found: { $talent }{ "\u000a" }  looked at: { $path }

m6a6f75726e616c2e636c692e74616c656e742e73686f772e7661726961626c6573 = variables:

m6a6f75726e616c2e636c692e74616c656e742e737461727475702e65786563757461626c65 = talent failed: could not inspect current executable: { $error }

m6a6f75726e616c2e636c692e74616c656e742e737461727475702e7061636b616765 = talent failed: could not locate installed { "solstone" } package

m6a6f75726e616c2e636c692e74616c656e742e776f726b2e6475726174696f6e = invalid Work source duration: { $error }

m6a6f75726e616c2e636c692e74616c656e742e776f726b2e6475726174696f6e2e72616e6765 = Work source duration is out of range

m6a6f75726e616c2e636c692e74616c656e742e776f726b2e696e74657276616c = invalid Work source interval: { $error }

m6a6f75726e616c2e636c692e74616c656e742e776f726b2e696e74657276616c2e72616e6765 = Work source interval is out of range

m6a6f75726e616c2e636c692e74616c656e742e776f726b2e6b6579 = invalid Work source segment: { $segment }

m6a6f75726e616c2e636c692e74616c656e742e776f726b2e72656164 = cannot read Work segment results: { $error }

m6a6f75726e616c2e636c692e74616c656e742e776f726b2e756e617661696c61626c65 = Work source segment is unavailable: { $day }/{ $segment }

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e626c6f636b6564 = activity blocked

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e626f64792e64617461 = this activity comes from a { "Strava" } workout, which is body data; nothing further runs on it.

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e6461792e6d697373696e67 = activity day file is missing: { $path }

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e66696e6765727072696e742e6661696c6564 = cannot fingerprint activity inputs

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e6d616c666f726d6564 = the facet declaration is malformed. correct it, then retry with { "--reactivate" }.

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e6d697373696e67 = the destination facet is missing. after restoring its declaration, retry with { "--reactivate" }; otherwise reclassify the source segment.

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e6d757465642e626c6f636b6564 = the destination facet is muted. unmute it, then retry the blocked activity with { "--reactivate" }.

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e6d757465642e7265747279 = the destination facet is muted. unmute it to resume unfinished activity processing.

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e6f776e65722e6669656c642e68656c64 = activity output is held by owner

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e70656e64696e67 = activity retry pending

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e726561642e626c6f636b6564 = the facet declaration could not be read. after restoring access, retry the blocked activity with { "--reactivate" }.

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e726561642e7265747279 = the facet declaration could not be read. the unfinished activity will retry automatically.

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e7265636f72642e6d697373696e67 = no activity record exists for this classification. after correcting the declaration, reprocess the source segment.

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e7265706169722e7265717569726564 = requires repair

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e7265706c61636564 = the destination facet was replaced. reclassify the source segment; an old result cannot be applied to the replacement.

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e72657472792e6964656e74697479 = activity work identity mismatch

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e72657472792e6b696e64 = not activity work

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e72657472792e6d75746564 = the activity's facet is muted

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e726f7574696e67 = correct the declaration, then reprocess this source segment.

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e726f772e6d697373696e67 = activity row is missing: { $path }

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e736574746c6564 = { $facet }/{ $activity } settled: this activity comes from a { "Strava" } workout, which is body data; nothing further runs on it.

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e7374617465 = activity state not saved: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e73746174652e7365676d656e74 = { $segment }: activity state not saved: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e776f726b2e6d616c666f726d6564 = malformed activity work: { $path }

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e776f726b2e70656e64696e67 = activity work pending: { $failures }

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e776f726b2e72756e6e696e67 = previous activity input is still running

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e776f726b2e756e7265616461626c65 = cannot read activity work { $path }: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e61637469766974792e776f726b2e76657273696f6e = unsupported activity work version

m6a6f75726e616c2e636c692e7468696e6b2e617267756d656e742e696e7465676572 = argument { $option }: invalid int value

m6a6f75726e616c2e636c692e7468696e6b2e617267756d656e742e756e6b6e6f776e = unrecognized arguments: { $argument }

m6a6f75726e616c2e636c692e7468696e6b2e617267756d656e742e76616c7565 = argument { $option }: expected one argument

m6a6f75726e616c2e636c692e7468696e6b2e63616e63656c6c6564 = thinking cancelled

m6a6f75726e616c2e636c692e7468696e6b2e6461696c792e626c6f636b6572732e6d6f7265 = { $names }, +{ $count } more

m6a6f75726e616c2e636c692e7468696e6b2e6461696c792e696e636f6d706c657465 = { $i18nCategory ->
    [one] { $i18nCategory-blockers ->
        [one] { "daily_incomplete" } ({ NUMBER($units, useGrouping: 0) } daily unit non-terminal; { NUMBER($blockers, useGrouping: 0) } segment blocker remains{ $details })
       *[other] { "daily_incomplete" } ({ NUMBER($units, useGrouping: 0) } daily unit non-terminal; { NUMBER($blockers, useGrouping: 0) } segment blocker(s) remain{ $details })
    }
   *[other] { $i18nCategory-blockers ->
        [one] { "daily_incomplete" } ({ NUMBER($units, useGrouping: 0) } daily unit(s) non-terminal; { NUMBER($blockers, useGrouping: 0) } segment blocker remains{ $details })
       *[other] { "daily_incomplete" } ({ NUMBER($units, useGrouping: 0) } daily unit(s) non-terminal; { NUMBER($blockers, useGrouping: 0) } segment blocker(s) remain{ $details })
    }
}

m6a6f75726e616c2e636c692e7468696e6b2e6461696c792e696e636f6d706c6574652e7265636f7264 = { "daily_incomplete" } ({ $units } daily unit(s) non-terminal; { $blockers } segment blocker(s) remain{ $details })

m6a6f75726e616c2e636c692e7468696e6b2e6461696c792e696e707574732e6368616e676564 = raw input changed during whole-day processing

m6a6f75726e616c2e636c692e7468696e6b2e6461696c792e6c6f636174696f6e2e7061636b6574 = daily location reuse has no stored packet

m6a6f75726e616c2e636c692e7468696e6b2e6461696c792e6c6f636174696f6e2e72656164 = daily location could not be read

m6a6f75726e616c2e636c692e7468696e6b2e6461696c792e736e617073686f742e6368616e676564 = daily evidence changed before preparation

m6a6f75726e616c2e636c692e7468696e6b2e6461696c792e736e617073686f742e6368616e6765642e6166746572 = daily evidence or contract changed during preparation

m6a6f75726e616c2e636c692e7468696e6b2e6461696c792e736e617073686f742e667265657a65 = daily input preparation: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e6461696c792e736e617073686f742e706172656e74 = talent root has no parent

m6a6f75726e616c2e636c692e7468696e6b2e6461696c792e736e617073686f742e72656d6f766564 = daily talent removed during preparation

m6a6f75726e616c2e636c692e7468696e6b2e6572726f72 = { "solstone journal think" }: { $message }

m6a6f75726e616c2e636c692e7468696e6b2e6572726f722e776974682e6c6f67 = { $primary }; { $log }

m6a6f75726e616c2e636c692e7468696e6b2e6865616c74682e6461792e696e76616c6964 = invalid day { $day }; expected { "YYYYMMDD" }

m6a6f75726e616c2e636c692e7468696e6b2e6865616c74682e6469726563746f7279 = cannot read directory { $path }: { $detail }

m6a6f75726e616c2e636c692e7468696e6b2e6865616c74682e6d61726b6572 = health marker error: { $detail }

m6a6f75726e616c2e636c692e7468696e6b2e6865616c74682e6d61726b65722e61746f6d6963 = health marker publication failed: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e6865616c74682e6d61726b65722e66696e6765727072696e74 = health marker fingerprint read failed: { $detail }

m6a6f75726e616c2e636c692e7468696e6b2e6865616c74682e6d61726b65722e696f = health marker I/O failed at { $path }: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e6865616c74682e6d61726b65722e6c6f636b = health marker lock failed: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e6865616c74682e6d61726b65722e6d616c666f726d6564 = health marker is malformed: { $path }

m6a6f75726e616c2e636c692e7468696e6b2e6865616c74682e6d61726b65722e73747265616d = stream health marker is malformed

m6a6f75726e616c2e636c692e7468696e6b2e6865616c74682e6d65746164617461 = cannot read file metadata { $path }: { $detail }

m6a6f75726e616c2e636c692e7468696e6b2e6865616c74682e70726f67726573732e756e7265616461626c65 = segment progress contains unreadable records

m6a6f75726e616c2e636c692e7468696e6b2e6865616c74682e72656164 = journal I/O error: { $detail }

m6a6f75726e616c2e636c692e7468696e6b2e6865616c74682e736f75726365 = health-log source error: { $detail }

m6a6f75726e616c2e636c692e7468696e6b2e68656c70 = usage: { "solstone journal think [-h] [--day DAY] [--segment SEGMENT] [--refresh] [--from-scratch] [--segments] [--facet NAME] [--activity ID] [--reactivate] [--stream STREAM] [--flush] [--settle] [-j N] [--no-timeout] [--segment-workers N] [--no-activity-prompts] [--skip-talents SKIP_TALENTS] [--live] [--updated] [--weekly] [--cadence] [--dry-run] [--sense-batch] [-v] [-d]" }

m6a6f75726e616c2e636c692e7468696e6b2e686f73742e73656e73652e6d697373696e67 = hosted Sense executor is unavailable

m6a6f75726e616c2e636c692e7468696e6b2e696e74656e742e676f6e65 = destructive request is no longer waiting

m6a6f75726e616c2e636c692e7468696e6b2e6c6f672e61637469766974792e6a6f696e = { "think" }: late { $day }/{ $segment } joined its open activity

m6a6f75726e616c2e636c692e7468696e6b2e6c6f672e61637469766974792e6e6f2e616476616e6365 = { "think" }: { $day }/{ $segment } does not advance live activity state

m6a6f75726e616c2e636c692e7468696e6b2e6c6f672e61637469766974792e7265706c6179 = { "think" }: late { $day }/{ $segment } is replayed with its stream's day

m6a6f75726e616c2e636c692e7468696e6b2e6c6f672e61637469766974792e7475726e = advancing { $stream } activity state without its turn: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e6c6f672e627564676574 = think outcome exceeds operational log record budget

m6a6f75726e616c2e636c692e7468696e6b2e6c6f672e636164656e63652e72656164 = Failed to load cadence state: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e6c6f672e636164656e63652e7365742e6173696465 = Failed to load cadence state: corrupt artifact set aside

m6a6f75726e616c2e636c692e7468696e6b2e6c6f672e6461696c792e736561726368 = could not inspect search work before daily thinking: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e6c6f672e6461792e666f726d6174 = day must be { "YYYYMMDD" }

m6a6f75726e616c2e636c692e7468696e6b2e6c6f672e6461792e6c6f63616c = invalid local day

m6a6f75726e616c2e636c692e7468696e6b2e6c6f672e646973706c61792e6d65746164617461 = could not save display metadata for { $path }: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e6c6f672e6661696c6564 = think run log { $path } failed: { $detail }

m6a6f75726e616c2e636c692e7468696e6b2e6c6f672e6661696c7572652e6c6f636b = think run log failure lock poisoned

m6a6f75726e616c2e636c692e7468696e6b2e6c6f672e68656c642e72656c65617365 = could not release held { $name } for { $segment }: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e6c6f672e6c6f636b = think run log lock poisoned

m6a6f75726e616c2e636c692e7468696e6b2e6c6f672e6e616d652e616e7377657273 = name answers: { $answers } changed, { $judged } activities judged again, { $changed } changed, { $waiting } waiting

m6a6f75726e616c2e636c692e7468696e6b2e6c6f672e70726f7669646572 = { $i18nCategory ->
    [one] thinking provider is unavailable; { NUMBER($left, useGrouping: 0) } segment waits for the next run
   *[other] thinking provider is unavailable; { NUMBER($left, useGrouping: 0) } segments wait for the next run
}

m6a6f75726e616c2e636c692e7468696e6b2e6c6f672e70726f76696465722e7265636f7264 = thinking provider is unavailable; { $left } segments wait for the next run

m6a6f75726e616c2e636c692e7468696e6b2e6c6f672e7265636f6e63696c696174696f6e = retry reconciliation failed: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e6c6f672e7365676d656e742e7475726e = thinking { $day }/{ $segment } without its turn: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e6c6f672e776f726b2e646973636f76657279 = work discovery: pass took { $elapsed } ms over { $inspected } of { $days } days; ranked { $handed } tasks for the queue

m6a6f75726e616c2e636c692e7468696e6b2e6d61726b646f776e2e656e746974696573 = Sense Entities

m6a6f75726e616c2e636c692e7468696e6b2e6d61726b646f776e2e656e74697479 = { $kind } — { $name } (role={ $role }, source={ $source }) — { $context }

m6a6f75726e616c2e636c692e7468696e6b2e6d61726b646f776e2e6f626a6563742e626f7468 = { $i18nCategory ->
    [one] { $i18nCategory-photos ->
        [one] { $class }: in { NUMBER($frames, useGrouping: 0) } frame and { NUMBER($photos, useGrouping: 0) } photo
       *[other] { $class }: in { NUMBER($frames, useGrouping: 0) } frame and { NUMBER($photos, useGrouping: 0) } photos
    }
   *[other] { $i18nCategory-photos ->
        [one] { $class }: in { NUMBER($frames, useGrouping: 0) } frames and { NUMBER($photos, useGrouping: 0) } photo
       *[other] { $class }: in { NUMBER($frames, useGrouping: 0) } frames and { NUMBER($photos, useGrouping: 0) } photos
    }
}

m6a6f75726e616c2e636c692e7468696e6b2e6d61726b646f776e2e6f626a6563742e626f74682e6d6178696d756d = { $i18nCategory ->
    [one] { $i18nCategory-photos ->
        [one] { $class }: in { NUMBER($frames, useGrouping: 0) } frame and { NUMBER($photos, useGrouping: 0) } photo, up to { $most } at once
       *[other] { $class }: in { NUMBER($frames, useGrouping: 0) } frame and { NUMBER($photos, useGrouping: 0) } photos, up to { $most } at once
    }
   *[other] { $i18nCategory-photos ->
        [one] { $class }: in { NUMBER($frames, useGrouping: 0) } frames and { NUMBER($photos, useGrouping: 0) } photo, up to { $most } at once
       *[other] { $class }: in { NUMBER($frames, useGrouping: 0) } frames and { NUMBER($photos, useGrouping: 0) } photos, up to { $most } at once
    }
}

m6a6f75726e616c2e636c692e7468696e6b2e6d61726b646f776e2e6f626a6563742e6672616d6573 = { $i18nCategory ->
    [one] { $class }: in { NUMBER($frames, useGrouping: 0) } frame
   *[other] { $class }: in { NUMBER($frames, useGrouping: 0) } frames
}

m6a6f75726e616c2e636c692e7468696e6b2e6d61726b646f776e2e6f626a6563742e6672616d65732e6d6178696d756d = { $i18nCategory ->
    [one] { $class }: in { NUMBER($frames, useGrouping: 0) } frame, up to { $most } at once
   *[other] { $class }: in { NUMBER($frames, useGrouping: 0) } frames, up to { $most } at once
}

m6a6f75726e616c2e636c692e7468696e6b2e6d61726b646f776e2e6f626a6563742e70686f746f73 = { $i18nCategory ->
    [one] { $class }: in { NUMBER($photos, useGrouping: 0) } photo
   *[other] { $class }: in { NUMBER($photos, useGrouping: 0) } photos
}

m6a6f75726e616c2e636c692e7468696e6b2e6d61726b646f776e2e6f626a6563742e70686f746f732e6d6178696d756d = { $i18nCategory ->
    [one] { $class }: in { NUMBER($photos, useGrouping: 0) } photo, up to { $most } at once
   *[other] { $class }: in { NUMBER($photos, useGrouping: 0) } photos, up to { $most } at once
}

m6a6f75726e616c2e636c692e7468696e6b2e6d61726b646f776e2e6f626a65637473 = Objects

m6a6f75726e616c2e636c692e7468696e6b2e6d61726b646f776e2e7061727469616c2e636f756e7473 = Left out: { $details }

m6a6f75726e616c2e636c692e7468696e6b2e6d61726b646f776e2e7061727469616c2e696e707574 = Left out: Some input did not fit. This result was written from part of the input.

m6a6f75726e616c2e636c692e7468696e6b2e6d61726b646f776e2e7061727469616c2e73746f7279 = This story, and anything saved with it, was written from part of the activity, not all of it.

m6a6f75726e616c2e636c692e7468696e6b2e6d61726b646f776e2e71756f746573 = Quotes

m6a6f75726e616c2e636c692e7468696e6b2e6d61726b646f776e2e73656e73652e656e746974696573 = ### Entities

m6a6f75726e616c2e636c692e7468696e6b2e6d61726b646f776e2e73656e73652e666163657473 = ### Facets

m6a6f75726e616c2e636c692e7468696e6b2e6d61726b646f776e2e73656e73652e7265636f7264 = ## Sense: { $kind }

m6a6f75726e616c2e636c692e7468696e6b2e6d61726b646f776e2e73656e73652e737065616b657273 = **Speakers:** { $names }

m6a6f75726e616c2e636c692e7468696e6b2e6d61726b646f776e2e737065616b6572 = Speaker { $number }

m6a6f75726e616c2e636c692e7468696e6b2e6d61726b646f776e2e7465726d696e616c2e636c6f636b = { $clock } · terminal

m6a6f75726e616c2e636c692e7468696e6b2e6d61726b646f776e2e7465726d696e616c2e6e6f74696365 = text shown in your terminal. this says what was visible, not what you read, typed or did. masking passwords and other secrets is best effort; your redaction rules do not apply.

m6a6f75726e616c2e636c692e7468696e6b2e6d61726b646f776e2e746f70696373 = Topics

m6a6f75726e616c2e636c692e7468696e6b2e6f7574636f6d652e636f6d706c65746564 = { "solstone journal think" }: { $count } completed

m6a6f75726e616c2e636c692e7468696e6b2e6f7574636f6d652e6661696c6564 = { "solstone journal think" }: { $count } failed

m6a6f75726e616c2e636c692e7468696e6b2e70686173652e6461696c792e70726572657175697369746573 = daily work is waiting for its segment prerequisites

m6a6f75726e616c2e636c692e7468696e6b2e70686173652e65786974 = exit { $code }

m6a6f75726e616c2e636c692e7468696e6b2e70686173652e7365676d656e742e70726572657175697369746573 = segment prerequisites changed

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e61637469766974792e636f756e74 = segments: { $segments }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e61637469766974792e656d707479 = No agents match activity type '{ $kind }'

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e61637469766974792e68656164696e67_v72656672657368 = Day { $day } { "--activity" } { $activity } { "--facet" } { $facet } (refresh)

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e61637469766974792e68656164696e67_v6f7264696e617279 = Day { $day } { "--activity" } { $activity } { "--facet" } { $facet }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e61637469766974792e6d697373696e67 = Activity not found: { $activity } in facet '{ $facet }' on { $day }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e61637469766974792e74797065 = type:     { $kind }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e636164656e6365 = Day { $day } — cadence agents

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e636164656e63652e66697265 = { $i18nCategory ->
    [one] { $i18nCategory-activities ->
        [one] fire  { $name } — window: { NUMBER($segments, useGrouping: 0) } segment, { NUMBER($activities, useGrouping: 0) } activity
       *[other] fire  { $name } — window: { NUMBER($segments, useGrouping: 0) } segment, { NUMBER($activities, useGrouping: 0) } activity(ies)
    }
   *[other] { $i18nCategory-activities ->
        [one] fire  { $name } — window: { NUMBER($segments, useGrouping: 0) } segment(s), { NUMBER($activities, useGrouping: 0) } activity
       *[other] fire  { $name } — window: { NUMBER($segments, useGrouping: 0) } segment(s), { NUMBER($activities, useGrouping: 0) } activity(ies)
    }
}

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e636164656e63652e666972652e7265636f7264 = fire  { $name } — window: { $segments } segment(s), { $activities } activity(ies)

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e636164656e63652e69646c65 = no-op { $name } — no new work since last cadence run

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e636164656e63652e736b6970 = skip  { $name } — interval not elapsed ({ $seconds }s < { $minutes }m)

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e646179 = Day { $day }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e6461792e72656672657368 = Day { $day } (refresh)

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e6461792e7365676d656e74 = Day { $day } segment { $segment }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e6461792e7365676d656e742e72656672657368 = Day { $day } segment { $segment } (refresh)

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e656d7074792e636164656e6365 = No prompts for schedule: { "cadence" }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e656d7074792e7765656b6c79 = No prompts for schedule: { "weekly" }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e666c757368 = Day { $day } { "--flush" } segment { $segment }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e666c7573682e656d707479 = No flush-eligible agents

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e666c7573682e74616c656e74_v67656e6572617465 = gen  { $name }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e666c7573682e74616c656e74_v6167656e74 = agent  { $name }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e69646c65 = idle segments: write stubs only (unless { "--refresh" })

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e6f7263686573747261746f72 = Sense orchestrator (linear):

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e706f73742e696e6465786572 = Post-phase: { "solstone journal indexer --rescan" }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e706f73742e7374617473 = Post-phase: { "solstone journal journal-stats" }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e707265 = Pre-phase:  { "solstone journal sense --day" } { $day } { "-j" } { $jobs }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e7072696f72697479 = Priority { $priority }:

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e726570726f63657373 = { $i18nCategory ->
    [one] Day { $day } — re-process { NUMBER($count, useGrouping: 0) } segment
   *[other] Day { $day } — re-process { NUMBER($count, useGrouping: 0) } segments
}

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e726570726f636573732e7265636f7264 = Day { $day } — re-process { $count } segments

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e7363686564756c652e656d707479 = No prompts for schedule: { $schedule }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e7365676d656e742e726f77 = [{ $index }/{ $total }] { $segment } ({ $start }-{ $end }) stream={ $stream }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e736b6970 = skip { $name } — no activity: { $facets }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e7374617465 = activity state machine: updates per segment

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e737465702e656e746974696573_v6578697374696e67 = { $step }. { $name } ({ $kind }/{ $format }) (exists) — always for non-idle

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e737465702e656e746974696573_v6e6577 = { $step }. { $name } ({ $kind }/{ $format }) (new) — always for non-idle

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e737465702e656e746974696573_v706c61696e = { $step }. { $name } ({ $kind }/{ $format }) — always for non-idle

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e737465702e706f737369626c65_v6578697374696e67 = { $step }. { $name } ({ $kind }/{ $format }) (exists) — if a speech transcript (never gates the segment)

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e737465702e706f737369626c65_v6e6577 = { $step }. { $name } ({ $kind }/{ $format }) (new) — if a speech transcript (never gates the segment)

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e737465702e706f737369626c65_v706c61696e = { $step }. { $name } ({ $kind }/{ $format }) — if a speech transcript (never gates the segment)

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e737465702e73637265656e_v6578697374696e67 = { $step }. { $name } ({ $kind }/{ $format }) (exists) — if { "recommend.screen_record" }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e737465702e73637265656e_v6e6577 = { $step }. { $name } ({ $kind }/{ $format }) (new) — if { "recommend.screen_record" }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e737465702e73637265656e_v706c61696e = { $step }. { $name } ({ $kind }/{ $format }) — if { "recommend.screen_record" }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e737465702e73656e7365_v6578697374696e67 = { $step }. { $name } ({ $kind }/{ $format }) (exists) — mandatory

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e737465702e73656e7365_v6e6577 = { $step }. { $name } ({ $kind }/{ $format }) (new) — mandatory

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e737465702e73656e7365_v706c61696e = { $step }. { $name } ({ $kind }/{ $format }) — mandatory

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e737465702e737065616b65722e6174747269627574696f6e_v6578697374696e67 = { $step }. { $name } ({ $kind }/{ $format }) (exists) — if audio embeddings (the model layer only if { "recommend.speaker_attribution" })

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e737465702e737065616b65722e6174747269627574696f6e_v6e6577 = { $step }. { $name } ({ $kind }/{ $format }) (new) — if audio embeddings (the model layer only if { "recommend.speaker_attribution" })

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e737465702e737065616b65722e6174747269627574696f6e_v706c61696e = { $step }. { $name } ({ $kind }/{ $format }) — if audio embeddings (the model layer only if { "recommend.speaker_attribution" })

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e7461626c652e6661636574_v67656e657261746f725f6578697374696e67 = gen  { $name }/{ $facet } (exists)

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e7461626c652e6661636574_v67656e657261746f725f6e6577 = gen  { $name }/{ $facet } (new)

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e7461626c652e6661636574_v67656e657261746f725f706c61696e = gen  { $name }/{ $facet }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e7461626c652e6661636574_v6167656e745f6578697374696e67 = agent  { $name }/{ $facet } (exists)

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e7461626c652e6661636574_v6167656e745f6e6577 = agent  { $name }/{ $facet } (new)

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e7461626c652e6661636574_v6167656e745f706c61696e = agent  { $name }/{ $facet }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e7461626c652e74616c656e74_v67656e657261746f725f6578697374696e67 = gen  { $name } (exists)

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e7461626c652e74616c656e74_v67656e657261746f725f6e6577 = gen  { $name } (new)

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e7461626c652e74616c656e74_v67656e657261746f725f706c61696e = gen  { $name }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e7461626c652e74616c656e74_v6167656e745f6578697374696e67 = agent  { $name } (exists)

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e7461626c652e74616c656e74_v6167656e745f6e6577 = agent  { $name } (new)

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e7461626c652e74616c656e74_v6167656e745f706c61696e = agent  { $name }

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e746f74616c = { $i18nCategory ->
    [one] Total: { NUMBER($total, useGrouping: 0) } agent
   *[other] Total: { NUMBER($total, useGrouping: 0) } agents
}

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e746f74616c2e7265636f7264 = Total: { $total } agents

m6a6f75726e616c2e636c692e7468696e6b2e706c616e2e7765656b6c79 = Day { $day } — weekly agents

m6a6f75726e616c2e636c692e7468696e6b2e707265666c696768742e6469736b = Your journal has run out of space on its disk ({ $detail }). Repair: free up space on that disk, then start your journal again.

m6a6f75726e616c2e636c692e7468696e6b2e707265666c696768742e68656c64 = { "generation-lease-contended" }: { "owner_role=" }{ $role } { "owner_pid=" }{ $pid } { "owner_started_at=" }{ $started } { "install_generation_id=" }{ $generation } { "owner_age_seconds=" }{ $age }; use the supervised path or stop that process cleanly

m6a6f75726e616c2e636c692e7468696e6b2e707265666c696768742e68656c642e756e617661696c61626c65 = { "generation-lease-contended" }: { "owner_details=unavailable" }; use the supervised path or stop the current process cleanly

m6a6f75726e616c2e636c692e7468696e6b2e7072696f726974792e656e7669726f6e6d656e74 = invalid work priority: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e7072696f726974792e73796e746178 = invalid work priority; expected band 1–6 and a source time

m6a6f75726e616c2e636c692e7468696e6b2e7265706169722e7365676d656e742e6d697373696e67 = missing repair segment

m6a6f75726e616c2e636c692e7468696e6b2e726570726f636573732e7265636f7264 = could not record reprocess outcome: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e61637469766974792e6368616e676564 = activity retry permission changed

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e61637469766974792e72656c6561736564 = activity is no longer set aside

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e6461696c792e6368616e676564 = daily retry permission changed

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e6461696c792e6d697373696e67 = missing daily permission

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e6461696c792e72656c6561736564 = daily unit is no longer held

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e6461696c792e7465726d696e616c2e6d697373696e67 = missing daily terminal record

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e6461696c792e756e7265736f6c766564 = daily retry is still unresolved

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e6461696c792e756e75736564 = daily writer did not consume retry permission

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e6461792e6d69736d61746368 = retry day does not match

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e65766964656e63652e6d697373696e67 = retry evidence is missing

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e7065726d697373696f6e2e6368616e676564 = retry permission changed

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e7261772e636f6f7264696e6174652e696e76616c6964 = invalid native retry coordinate

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e7261772e636f6f7264696e6174652e75746638 = non-UTF8 retry coordinate

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e7261772e66696e6765727072696e742e696e76616c6964 = native failure input fingerprint is malformed

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e7261772e68616e646c65722e6d697373696e67 = native failure has no handler

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e7261772e6865616465722e696e636f6d706c657465 = native processing header is incomplete

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e7261772e6865616465722e6b696e64 = native processing header is not an object

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e7261772e6b696e64 = not native analysis

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e7261772e706172656e742e6d697373696e67 = native output has no parent

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e7261772e7065726d697373696f6e2e756e75736564 = native writer did not consume the retry permission

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e7261772e726563656970742e6368616e676564 = native terminal receipt changed

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e7261772e726563656970742e6d697373696e67 = native terminal receipt is missing

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e7261772e726563656970742e756e7075626c6973686564 = native writer has not published this retry's terminal receipt

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e7261772e726563656970742e756e7265616461626c65 = native terminal receipt is unreadable

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e7261772e7265636f72642e6d616c666f726d6564 = native processing record is malformed

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e7261772e72656c6561736564 = native output is no longer held

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e7261772e736368656d612e756e6b6e6f776e = native processing schema is unrecognized

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e7261772e736f757263652e696e76616c6964 = native failure has no safe source filename

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e7261772e736f757263652e6d69736d61746368 = native failure source does not match its output

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e7365676d656e742e6368616e676564 = segment retry permission changed

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e74616c656e742e64697361626c6564 = held talent is no longer enabled

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e74616c656e742e6469737061746368 = held talent could not be dispatched

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e74616c656e742e7365676d656e742e6d697373696e67 = segment no longer exists

m6a6f75726e616c2e636c692e7468696e6b2e72657472792e74616c656e742e736b6970706564 = held talent could not be retried

m6a6f75726e616c2e636c692e7468696e6b2e726f7574696e672e6c6f672e6d616c666f726d6564 = cannot read routing recovery from malformed health logs

m6a6f75726e616c2e636c692e7468696e6b2e726f7574696e672e736f757263652e696e76616c6964 = retained routing source is invalid

m6a6f75726e616c2e636c692e7468696e6b2e7365676d656e742e616d626967756f7573 = { $i18nCategory ->
    [one] ambiguous segment { $day }/{ $segment }: { NUMBER($count, useGrouping: 0) } stream shares this name
   *[other] ambiguous segment { $day }/{ $segment }: { NUMBER($count, useGrouping: 0) } streams share this name
}

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e636f6d706c657465 = Batch processing complete

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e636f6e6669672e636f6e63757272656e6379 = sense: invalid { $handler }.{ "max_concurrent" }; defaulting to 1

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e636f6e6669672e72656164 = sense: journal config unavailable: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e636f6e6669672e72756e74696d65 = sense: invalid { $handler }.{ "max_runtime" }; defaulting to { $seconds }s

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e636f6e666c696374 = { "--sense-batch" } is incompatible with { $options }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e646179 = { "--sense-batch" } requires { "--day" }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e6461792e6d697373696e67 = Day directory not found: { $path }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e6465666572726564 = { $i18nCategory ->
    [one] Batch processing deferred: { $deferred } of { $ran } handler waiting; unfinished files will retry
   *[other] Batch processing deferred: { $deferred } of { $ran } handlers waiting; unfinished files will retry
}

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e6572726f722e64656c657465 = could not delete { $path }: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e6572726f722e64656c657465642e6469727479 = reprocess removed { $path }, but could not durably mark day { $day } dirty: { $detail }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e6572726f722e6661696c6564 = { $failed } failed of { $ran } ran

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e6572726f722e68656c64 = sense batch lease for day { $day } is held by another process

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e6572726f722e6a6f696e = Sense worker did not stop cleanly

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e6572726f722e6c65617365 = sense batch lease error: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e6572726f722e72756e74696d65 = batch callosum runtime unavailable

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e6572726f722e7363616e = could not scan { $path }: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e6572726f722e74696d656f7574 = sense batch timed out after { $duration }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e66696c6573 = { $i18nCategory ->
    [one] Found { $count } unprocessed file to process ({ $extensions })
   *[other] Found { $count } unprocessed files to process ({ $extensions })
}

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e696e7465727275707465642e7772697465 = failed to record { "observe.interrupted" } event to { $path }: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e6c656173652e736b6970 = Sense batch lease for day { $day } is held; skipping batch walk

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e6e6f7468696e67 = No unprocessed files found in { $path }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e7072696f726974792e656e636f64696e67 = { $variable } contains invalid UTF-8

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e70726f63657373696e672e646179 = { $i18nCategory ->
    [one] Processing files from day { $day } with { $jobs } concurrent job
   *[other] Processing files from day { $day } with { $jobs } concurrent jobs
}

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e70726f63657373696e672e7365676d656e74 = { $i18nCategory ->
    [one] Processing files from day { $day } (segment: { $segment }) with { $jobs } concurrent job
   *[other] Processing files from day { $day } (segment: { $segment }) with { $jobs } concurrent jobs
}

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e72656675736564 = sense batch refused: { $reason }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e72657472792e6368616e676564 = native retry inputs changed

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e72657472792e68616e646c6572 = unknown native retry handler

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e72657472792e6b696e64 = retry is not native analysis

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e72657472792e6d69736d61746368 = native failure handler does not match its source

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e617574686f72697479 = { $handler } launch authority was not managed

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e6465666572726564 = deferred (exit { $code })

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e64657461696c = sense: day={ $day } stream={ $stream } segment={ $segment } file={ $file } error={ $error }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e65786974 = { $handler } exit { $code }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e6661696c6564 = failed (exit { $code })

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e6c61756e6368 = { $handler } spawn failed

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e6c61756e63682e6661696c6564 = sense: { $handler } launch failed: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e6c6f672e7461696c = sense: handler diagnostic log={ $log } tail={ $tail }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e6c6f672e756e7265616461626c65 = sense: handler diagnostic log={ $log } unreadable: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e6f75747075742e696e636f6d706c657465 = { $handler } exited 0 with unfinished output (analysis output incomplete): { $path }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e6f75747075742e6e6f6e726567756c6172 = { $handler } exited 0 without a regular output: { $path }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e6f75747075742e756e66696e6973686564 = { $handler } exited 0 with unfinished output ({ $reason }): { $path }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e6f75747075742e756e7265616461626c65 = { $handler } exited 0 without readable output { $path }: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e706f6f6c = { $handler } pool unavailable

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e70726f6772616d2e65786563757461626c65 = { $handler } dispatcher current_exe failed: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e70726f6772616d2e6d697373696e67 = { $handler } dispatcher missing: { $path }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e70726f6772616d2e6e6f6e65786563757461626c65 = { $handler } dispatcher not executable: { $path }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e70726f6772616d2e6e6f6e726567756c6172 = { $handler } dispatcher not a file: { $path }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e737461747573 = sense: handler={ $handler } file={ $file } outcome={ $outcome } log={ $log }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e7375727669766f7273 = sense: process-tree-not-reaped handler={ $handler } reason={ $reason } survivors={ $count }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e7465726d696e6174696f6e = sense: managed termination failed handler={ $handler }: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e74696d656f7574 = { $handler } watchdog_timeout after { $seconds }s

m6a6f75726e616c2e636c692e7468696e6b2e73656e73652e776f726b65722e7761746368646f67 = watchdog or process wait failed

m6a6f75726e616c2e636c692e7468696e6b2e736574746c652e73617665 = activity settle state not saved: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e757064617465642e6361706162696c697479 = catchup capability unavailable on this platform

m6a6f75726e616c2e636c692e7468696e6b2e757064617465642e636f6e666c696374 = { "--updated" } is incompatible with { $options }

m6a6f75726e616c2e636c692e7468696e6b2e757064617465642e696f = catchup I/O failed at { $path }: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e757064617465642e6d61726b6572 = catchup marker error: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e757064617465642e70617468 = catchup journal path error: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e757064617465642e7374617465 = catchup state is malformed: { $detail }

m6a6f75726e616c2e636c692e7468696e6b2e75736167652e6572726f72 = usage: { "solstone journal think [-h] [--day DAY] [--segment SEGMENT] [--refresh] [--from-scratch] [--segments] [--facet NAME] [--activity ID] [--reactivate] [--stream STREAM] [--flush] [--settle] [-j N] [--no-timeout] [--segment-workers N] [--no-activity-prompts] [--skip-talents SKIP_TALENTS] [--live] [--updated] [--weekly] [--cadence] [--dry-run] [--sense-batch] [-v] [-d]" }{ "\u000a" }{ "solstone journal think" }: error: { $message }

m6a6f75726e616c2e636c692e7468696e6b2e76616c69646174696f6e2e61637469766974792e696e636f6d70617469626c65 = { "--activity" } is incompatible with { "--segment" }, { "--segments" }, and { "--flush" }

m6a6f75726e616c2e636c692e7468696e6b2e76616c69646174696f6e2e61637469766974792e72657175697265732e646179 = { "--activity" } requires { "--day" }

m6a6f75726e616c2e636c692e7468696e6b2e76616c69646174696f6e2e61637469766974792e72657175697265732e6661636574 = { "--activity" } requires { "--facet" }

m6a6f75726e616c2e636c692e7468696e6b2e76616c69646174696f6e2e636164656e63652e696e636f6d70617469626c65 = { "--cadence" } is incompatible with { "--segment" }, { "--segments" }, { "--activity" }, { "--flush" }, and { "--weekly" }

m6a6f75726e616c2e636c692e7468696e6b2e76616c69646174696f6e2e6461696c792e6f6e6c79 = { "--daily-only" } requires { "--day" } and cannot be combined with another thinking mode

m6a6f75726e616c2e636c692e7468696e6b2e76616c69646174696f6e2e66616365742e72657175697265732e6163746976697479 = { "--facet" } requires { "--activity" }

m6a6f75726e616c2e636c692e7468696e6b2e76616c69646174696f6e2e666c7573682e696e636f6d70617469626c65 = { "--flush" } is incompatible with { "--segments" } and { "--refresh" }

m6a6f75726e616c2e636c692e7468696e6b2e76616c69646174696f6e2e666c7573682e72657175697265732e7365676d656e74 = { "--flush" } requires { "--segment" }

m6a6f75726e616c2e636c692e7468696e6b2e76616c69646174696f6e2e6d756c74692e776f726b65722e756e6c696d697465642e6a6f6273 = { "--jobs" } 0 is incompatible with multi-worker { "--segments" }; set { "--jobs" } to a positive bound or { "--segment-workers" } 1

m6a6f75726e616c2e636c692e7468696e6b2e76616c69646174696f6e2e6e6f2e61637469766974792e70726f6d7074732e776974682e6163746976697479 = { "--no-activity-prompts" } cannot be combined with { "--activity" }

m6a6f75726e616c2e636c692e7468696e6b2e76616c69646174696f6e2e726561637469766174652e696e636f6d70617469626c652e72656672657368 = { "--reactivate" } is incompatible with { "--refresh" }

m6a6f75726e616c2e636c692e7468696e6b2e76616c69646174696f6e2e726561637469766174652e72657175697265732e6163746976697479 = { "--reactivate" } requires { "--activity" }

m6a6f75726e616c2e636c692e7468696e6b2e76616c69646174696f6e2e7265706169722e7365676d656e74 = { "--repair-segment" } requires { "--day" } and { "--segment" } and cannot be combined with another thinking mode

m6a6f75726e616c2e636c692e7468696e6b2e76616c69646174696f6e2e72657472792e756e6974 = { "--retry-unit" } requires { "--day" } and { "--retry-cause" } without another thinking mode

m6a6f75726e616c2e636c692e7468696e6b2e76616c69646174696f6e2e7365676d656e742e776f726b6572732e72616e6765 = { "--segment-workers" } must be between 1 and 32

m6a6f75726e616c2e636c692e7468696e6b2e76616c69646174696f6e2e7365676d656e74732e696e636f6d70617469626c65 = { "--segments" } is incompatible with { "--segment" } and { "--facet" }

m6a6f75726e616c2e636c692e7468696e6b2e76616c69646174696f6e2e736574746c652e696e636f6d70617469626c65 = { "--settle" } is incompatible with { "--segment" }, { "--segments" }, { "--activity" }, { "--flush" }, { "--refresh" }, { "--weekly" }, and { "--cadence" }

m6a6f75726e616c2e636c692e7468696e6b2e76616c69646174696f6e2e7765656b6c792e696e636f6d70617469626c65 = { "--weekly" } is incompatible with { "--segment" }, { "--segments" }, { "--activity" }, and { "--flush" }

m6a6f75726e616c2e636c692e7468696e6b2e76616c69646174696f6e2e776f726b2e696e74656e74 = { "--work-intent" } requires a from-scratch daily request

m6a6f75726e616c2e636c692e7468696e6b2e776169742e636f72746578 = cortex wait failed: { $error }

m6a6f75726e616c2e636c692e7468696e6b2e776169742e6661696c6564 = wait failed: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e617564696f2e6469726563746f7279 = Audio file must be in a segment directory ({ "HHMMSS_LEN/" }), but parent is: { $parent }

m6a6f75726e616c2e636c692e7472616e7363726962652e617564696f2e666f726d6174 = Unsupported audio format: { $extension }. Supported formats: { $formats }

m6a6f75726e616c2e636c692e7472616e7363726962652e617564696f2e6d697373696e67 = Audio file not found.{ "\u000a" }  Tried absolute:         { $absolute }{ "\u000a" }  Tried journal-relative: { $relative }

m6a6f75726e616c2e636c692e7472616e7363726962652e6261636b656e642e63686f696365 = argument { "--backend" }: invalid choice: '{ $backend }' (choose from { $choices })

m6a6f75726e616c2e636c692e7472616e7363726962652e6261636b656e642e6475706c6963617465 = argument { "--backend" }: may only be specified once

m6a6f75726e616c2e636c692e7472616e7363726962652e62617463682e646179 = { $i18nCategory ->
    [one] { "transcribe --all" }: { NUMBER($count, useGrouping: 0) } day could not be read
   *[other] { "transcribe --all" }: { NUMBER($count, useGrouping: 0) } day(s) could not be read
}

m6a6f75726e616c2e636c692e7472616e7363726962652e62617463682e6461792e7265636f7264 = { "transcribe --all" }: { $count } day(s) could not be read

m6a6f75726e616c2e636c692e7472616e7363726962652e62617463682e6661696c6564 = transcription: { $failed } failed

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e696e7075742e63616e63656c6c6564 = processing input verification cancelled

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e696e7075742e6368616e6765642e636f6d6d6974 = processing input changed before publication

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e696e7075742e6368616e6765642e6f70656e = processing input changed while opening

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e696e7075742e6368616e6765642e72656164 = processing input changed while reading

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e696e7075742e6e6f2e617474656d7074 = input has no output attempt

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e696e7075742e6e6f742e66696c65 = processing input is not a plain file

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e6c6f636b2e656e7472792e756e73616665 = persistent lock entry is an unsafe { $kind }: { $path }

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e6c6f636b2e696e76616c6964 = invalid persistent lock entry { $name }

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e6c6f636b2e6d697373696e67 = persistent lock parent is missing: { $path }

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e6c6f636b2e6d6f6465 = persistent lock entry has mode { $mode }, expected 600: { $path }

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e6c6f636b2e6e616d657370616365 = persistent lock entry changed during acquisition: { $path }

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e6c6f636b2e706172656e742e6368616e676564 = persistent lock parent changed during acquisition: { $path }

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e6c6f636b2e706172656e742e756e73616665 = persistent lock parent is an unsafe { $kind }: { $path }

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e706172656e742e6d697373696e67 = missing parent directory

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e726f6f742e616273656e74 = journal root does not exist

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e726f6f742e6162736f6c757465 = journal root must be absolute

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e726f6f742e616e636573746f72 = canonical journal root has a non-UTF-8 ancestor

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e726f6f742e63616e6f6e6963616c2e6162736f6c757465 = canonical journal root is not absolute

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e726f6f742e6368616e676564 = journal source changed during acquisition

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e726f6f742e636f6d706f6e656e74 = canonical journal root has no final component

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e726f6f742e6469726563746f7279 = journal root is not a directory

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e726f6f742e696e76616c6964 = invalid journal root { $path }: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e726f6f742e696f = { $operation }: { $reason }

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e726f6f742e756e737570706f72746564 = unsupported journal root { $path }: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e7365676d656e742e6e6f742e636f7665726564 = segment is not covered by these output attempts

m6a6f75726e616c2e636c692e7472616e7363726962652e636c61696d2e7461726765742e6e616d65 = missing lock target name

m6a6f75726e616c2e636c692e7472616e7363726962652e636f6e6669672e6261636b656e64 = invalid { "transcribe.backend" } in journal config; choose one of: { $backends }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e61737365742e616273656e74 = could not find nonempty model asset { $asset }; searched { $paths }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e61737365742e646967657374 = model asset { $asset } at { $path } has { "sha256" } { $actual }, expected { $expected }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e61737365742e65786563757461626c65 = could not determine current executable: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e61737365742e6f76657272696465 = { "SOLSTONE_TRANSCRIBE_MODEL_ASSETS_DIR" }={ $directory } does not provide nonempty model asset { $asset }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e617564696f2e627564676574 = decoded sample budget { $limit } exceeded for { $path }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e617564696f2e636f7272757074 = corrupt audio input { $path }: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e617564696f2e656d707479 = audio input is empty: { $path }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e617564696f2e66666d706567 = { "FFmpeg" } error for { $path }: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e617564696f2e666c757368 = { $i18nCategory ->
    [one] { $i18nCategory-flushed ->
        [one] resampler flush did not converge for { $path } after { NUMBER($iterations, useGrouping: 0) } iteration ({ NUMBER($flushed, useGrouping: 0) } sample flushed; { $remaining } still delayed)
       *[other] resampler flush did not converge for { $path } after { NUMBER($iterations, useGrouping: 0) } iteration ({ NUMBER($flushed, useGrouping: 0) } samples flushed; { $remaining } still delayed)
    }
   *[other] { $i18nCategory-flushed ->
        [one] resampler flush did not converge for { $path } after { NUMBER($iterations, useGrouping: 0) } iterations ({ NUMBER($flushed, useGrouping: 0) } sample flushed; { $remaining } still delayed)
       *[other] resampler flush did not converge for { $path } after { NUMBER($iterations, useGrouping: 0) } iterations ({ NUMBER($flushed, useGrouping: 0) } samples flushed; { $remaining } still delayed)
    }
}

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e617564696f2e696f = { "I/O" } error for { $path }: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e617564696f2e73616d706c652e72617465 = invalid { "WAV" } sample rate: { $rate }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e617564696f2e73616d706c6573 = no audio samples decoded from { $path }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e617564696f2e736964656361722e637265617465 = cannot create { "f32le" } sidecar { $path }: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e617564696f2e736964656361722e73796e63 = cannot sync { "f32le" } sidecar { $path }: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e617564696f2e736964656361722e7772697465 = cannot write { "f32le" } sidecar { $path }: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e617564696f2e73747265616d = no audio stream in { $path }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e617564696f2e7761762e73697a65 = { $i18nCategory ->
    [one] WAV data is too large for a { "RIFF u32" } field: { NUMBER($samples, useGrouping: 0) } sample
   *[other] WAV data is too large for a { "RIFF u32" } field: { NUMBER($samples, useGrouping: 0) } samples
}

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e6261636b656e642e6d656d6f7279 = no viable { "STT" } backend (available memory: { $available }, local floor: { $floor })

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e6261636b656e642e706c6174666f726d = { "STT" } backend is unavailable on this host platform: { $backend }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e63616e63656c6c6564 = embedded transcription cancelled before publication

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636c61696d = could not acquire transcription ownership for { $path }: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e636f6e666964656e7469616c2e617564696f2e68616e646c696e672e69732e64697361626c6564 = confidential audio handling is disabled

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e636f6e666964656e7469616c2e6c616e652e626c6f636b732e7374742e6261636b656e642e6261636b656e642e7261772e617564696f = confidential lane blocks { "STT" } backend { $backend }; raw audio must stay local

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e6661696c757265 = confidential transcription { $reason }: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e686f737465642e7374742e72657475726e65642e687474702e737461747573 = hosted { "STT" } returned { "HTTP" } { $status }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e686f737465642e7374742e72657475726e65642e687474702e756e617574686f72697a6564 = hosted { "STT" } returned { "HTTP" } 401

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e7468652e636f6e666964656e7469616c2e61646d697373696f6e2e70726f62652e6661696c6564 = the confidential admission probe failed

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e7468652e636f6e666964656e7469616c2e6174746573746174696f6e2e6368616e6e656c2e69732e6e6f742e7265616479 = the confidential attestation channel is not ready

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e7468652e636f6e666964656e7469616c2e656e64706f696e742e6861732e6e6f2e63726564656e7469616c = the confidential endpoint has no credential

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e7468652e636f6e666964656e7469616c2e6c616e652e6861732e6e6f2e636f6e666964656e7469616c2e62796f2e656e64706f696e74 = the confidential lane has no confidential { "BYO" } endpoint

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e7468652e636f6e666964656e7469616c2e6c616e652e69732e6e6f2e6c6f6e6765722e616374697665 = the confidential lane is no longer active

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e776f72642e612e776f72642e656e7472792e7761732e626c616e6b = a word entry was blank

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e776f72642e612e776f72642e656e7472792e7761732e6d697373696e672e612e72657175697265642e6b6579 = a word entry was missing a required key

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e776f72642e612e776f72642e656e7472792e7761732e6d697373696e672e656e64 = a word entry was missing `end`

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e776f72642e612e776f72642e656e7472792e7761732e6d697373696e672e7374617274 = a word entry was missing `start`

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e776f72642e612e776f72642e656e7472792e7761732e6d697373696e672e776f7264 = a word entry was missing `word`

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e776f72642e612e776f72642e656e7472792e7761732e6e6f742e616e2e6f626a656374 = a word entry was not an object

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e776f72642e636f6e7472616374 = hosted { "STT" } response violated the verbose { "JSON" } contract: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e776f72642e726573706f6e73652e6861642e6e6f2e776f7264732e6172726179 = response had no words array

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e776f72642e726573706f6e73652e6861642e746578742e6275742e6e6f2e776f72642e74696d696e6773 = response had text but no word timings

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e776f72642e726573706f6e73652e7761732e6e6f742e616e2e6f626a656374 = response was not an object

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e776f72642e726573706f6e73652e7761732e6e6f742e6a736f6e = response was not { "JSON" }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f6e666964656e7469616c2e776f72642e776f72642e6b65792e7761732e6e6f742e612e66696e6974652e6e756d6265722e666f756e64 = word `{ $key }` was not a finite number (found { $found })

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f72656d6c2e6661696c757265 = { "parakeet" } { "CoreML" } { $reason }: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f72656d6c2e68656c7065722e6a736f6e2e7761732e6e6f742e616e2e6f626a656374 = helper { "JSON" } was not an object

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f72656d6c2e68656c7065722e726573706f6e73652e6861732e696e76616c69642e6b6579 = helper response has invalid { $key }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f72656d6c2e68656c7065722e726573706f6e73652e6d697373696e672e6b6579 = helper response missing { $key }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f72656d6c2e68656c7065722e726573706f6e73652e6d697373696e672e746f6b656e2e74696d696e6773 = helper response missing { "token_timings[]" }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f72656d6c2e68656c7065722e72657475726e65642e7472616e7363726970742e746578742e776974686f75742e746f6b656e2e74696d696e6773 = helper returned transcript text without token timings

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f72656d6c2e68656c7065722e746f6b656e2e74696d696e67732e636f6c6c61707365642e746f2e6e6f2e776f726473 = helper token timings collapsed to no words

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f72656d6c2e68656c7065722e76657273696f6e2e6a736f6e2e7761732e6e6f742e616e2e6f626a656374 = helper version { "JSON" } was not an object

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f72656d6c2e68656c7065722e76657273696f6e2e726573706f6e73652e6d697373696e672e6669656c64 = helper version response missing { $field }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f72656d6c2e6d697373696e672e7374646572722e70697065 = missing { "stderr" } pipe

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f72656d6c2e6d697373696e672e7374646f75742e70697065 = missing { "stdout" } pipe

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f72656d6c2e6e616d652e7265616465722e70616e69636b6564 = { $name } reader panicked

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f72656d6c2e706172616b6565742e636f72656d6c2e6d6f64656c2e69732e6e6f742e696e7374616c6c65642e7965742e70617468 = { "Parakeet" } { "CoreML" } model is not installed yet: { $path }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f72656d6c2e706172616b6565742e68656c7065722e6578697465642e776974682e737461747573 = { "Parakeet" } helper exited with { $status }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f72656d6c2e706172616b6565742e68656c7065722e69732e6d697373696e672e70617468 = { "Parakeet" } helper is missing: { $path }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f72656d6c2e706172616b6565742e68656c7065722e69732e6e6f742e612e66696c652e70617468 = { "Parakeet" } helper is not a file: { $path }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f72656d6c2e706172616b6565742e68656c7065722e69732e6e6f742e65786563757461626c652e70617468 = { "Parakeet" } helper is not executable: { $path }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f72656d6c2e706172616b6565742e68656c7065722e74696d65642e6f7574 = { "Parakeet" } helper timed out

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f72656d6c2e706172616b6565742e68656c7065722e74696d65642e6f75742e61667465722e7365636f6e64732e73 = { "Parakeet" } helper timed out after { $seconds }s

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f72656d6c2e7075626c69636174696f6e2e62757379 = { "parakeet" } { "CoreML" } publication lock busy

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e636f72656d6c2e746f6b656e2e74696d696e672e6d7573742e62652e616e2e6f626a656374 = token timing must be an object

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e6370702e6465766963652e6d7573742e62652e6f6e652e6f662e6175746f2e637075 = device must be one of: { "auto, cpu" }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e6370702e6661696c757265 = { "parakeet.cpp" } { $reason }: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e6370702e687474702e737461747573 = { "HTTP" } { $status }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e6370702e706172616b6565742e6370702e69732e6f6e6c792e737570706f727465642e6f6e2e6c696e75782e616e64 = { "parakeet-cpp" } is only supported on linux and windows

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e6370702e706172616b6565742e7365727665722e63726564656e7469616c732e6172652e756e617661696c61626c652e696e2e746869732e70726f63657373 = { "Parakeet" } server credentials are unavailable in this process.

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e6370702e706172616b6565742e7365727665722e69732e6e6f742e72656164792e796574 = { "Parakeet" } server is not ready yet.

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e6370702e726573706f6e73652e6861732e746578742e6275742e6e6f2e776f72642e74696d696e6773 = response has text but no word timings

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e6370702e726573706f6e73652e6a736f6e2e7761732e696e76616c69642e6572726f72 = response { "JSON" } was invalid: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e6370702e726573706f6e73652e6a736f6e2e7761732e6e6f742e616e2e6f626a656374 = response { "JSON" } was not an object

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e6370702e726573706f6e73652e6d697373696e672e746f702e6c6576656c2e776f726473 = response missing top-level { "words[]" }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e6370702e746c732e69732e7265717569726564 = { "TLS" } is required

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e6370702e7472616e736372697074696f6e2e726571756573742e74696d65642e6f7574 = transcription request timed out

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e6370702e776f72642e74696d696e672e636f6e7461696e732e696e76616c69642e6e756d657269632e76616c7565 = word timing contains invalid numeric value

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e6370702e776f72642e74696d696e672e6974656d2e6d7573742e62652e616e2e6f626a656374 = word timing item must be an object

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e6370702e776f72642e74696d696e672e6d697373696e672e6b65792e6b6579 = word timing missing key: { $key }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e6370702e776f72642e74696d696e672e746578742e7761732e626c616e6b = word timing text was blank

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e696e7075742e696e7370656374 = could not inspect input { $path }: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e6f727068616e = could not remove orphan { "NPZ" } { $path }: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7061796c6f61642e706172656e74 = transcript destination has no parent

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e70726f636573732e61646d697373696f6e = hosted launch admission failed: { $reason }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e70726f636573732e6361706162696c697479 = host capability unavailable: { $needed }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e70726f636573732e636f6d6d616e64 = cannot spawn an empty command

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e70726f636573732e636f6e6669726d = post-spawn confirmation failed for pid { $pid }: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e70726f636573732e6578706c69636974 = this authority is not explicitly unowned

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e70726f636573732e6964656e74697479 = failed to capture birth-bound identity for spawned pid { $pid }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e70726f636573732e6c6f67 = failed to prepare operational log: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e70726f636573732e6f7574707574 = child output is unavailable

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e70726f636573732e737061776e = failed to spawn child: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e70726f636573732e7465726d696e617465 = failed to terminate child: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e70726f636573732e756e6f776e65642e726561736f6e = { "ExplicitlyUnowned" } reason must be nonempty

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7075626c6973682e6f7574707574 = { "output.jsonl_path" } is required

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7075626c6973682e706172656e74 = output has no parent

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7075626c6973682e726177 = { "header.raw" } is required

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7075626c6973682e7369626c696e67 = processing raw must be a sibling filename

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e737065616b65722e616e616c79736973 = speaker analysis failed: { $stage }/{ $reason }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e73746174656d656e742e6669656c64 = full transcript statement has no valid { $field }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7465726d696e616c2e7061796c6f6164 = could not prepare terminal payload: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7465726d696e616c2e7061796c6f61642e70617468 = could not prepare or remove terminal payload { $path }: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7465726d696e616c2e72657175657374 = could not serialize terminal writer request: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7472616e7363726970742e7061796c6f6164 = could not prepare transcript payload: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7472616e7363726970742e7061796c6f61642e70617468 = could not prepare or remove transcript payload { $path }: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7472616e7363726970742e72657175657374 = could not serialize transcript writer request: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e636f6e74726163742e65786974 = { "VAD" } helper contract error (exit { $code }): { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e636f6e74726163742e657869742e737464657272 = { "VAD" } helper contract error (exit { $code }): { $detail }: { $stderr }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e636f6e74726163742e7369676e616c = { "VAD" } helper contract error (no exit code): { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e636f6e74726163742e7369676e616c2e737464657272 = { "VAD" } helper contract error (no exit code): { $detail }: { $stderr }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e636f756c642e6e6f742e6372656174652e7661642e696e7075742e6469726563746f72792e6572726f72 = could not create { "VAD" } input directory: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e636f756c642e6e6f742e72656d6f76652e7661642e696e7075742e6469726563746f72792e6572726f72 = could not remove { "VAD" } input directory: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e636f756c642e6e6f742e72656d6f76652e7661642e696e7075742e6469726563746f72792e696e7075742e72656d61696e732e696e = could not remove { "VAD" } input directory: input remains in use

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e636f756c642e6e6f742e73656e642e7661642e726571756573742e6572726f72 = could not send { "VAD" } request: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e636f756c642e6e6f742e73657269616c697a652e7661642e726571756573742e6572726f72 = could not serialize { "VAD" } request: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e6572726f722e656d707479 = { "VAD" } error response was empty

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e6572726f722e6d616e79 = { "VAD" } error response contained more than one line

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e6572726f722e75746638 = { "VAD" } error response was not { "UTF-8" }: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e68656c706572 = { "VAD" } helper { $reason } (exit { $code }): { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e6c61756e63682e6661696c6564 = could not launch { "VAD" } helper { $path }: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e726573706f6e73652e656d707479 = { "VAD" } response was empty

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e726573706f6e73652e6d616e79 = { "VAD" } response contained more than one line

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e726573706f6e73652e75746638 = { "VAD" } response was not { "UTF-8" }: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e7661642e6572726f722e726573706f6e73652e6c61636b732e736368656d612e726561736f6e2e6f722e64657461696c = { "VAD" } error response lacks schema, reason, or detail

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e7661642e6572726f722e726573706f6e73652e6d7573742e62652e616e2e6f626a656374 = { "VAD" } error response must be an object

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e7661642e6572726f722e726573706f6e73652e76696f6c617465732e6974732e736368656d61 = { "VAD" } error response violates its schema

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e7661642e6572726f722e726573706f6e73652e7761732e6e6f742e6a736f6e2e6572726f72 = { "VAD" } error response was not { "JSON" }: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e7661642e68656c7065722e737464696e2e7761732e756e617661696c61626c65 = { "VAD" } helper { "stdin" } was unavailable

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e7661642e68656c7065722e7465726d696e617465642e776974686f75742e616e2e657869742e636f6465 = { "VAD" } helper terminated without an exit code

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e7661642e726573706f6e73652e6861732e616e2e756e6b6e6f776e2e736368656d61 = { "VAD" } response has an unknown schema

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e7661642e726573706f6e73652e6861732e6e6f2e626f6f6c65616e2e6861732e737065656368 = { "VAD" } response has no boolean { "has_speech" }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e7661642e726573706f6e73652e6861732e6e6f2e6e6f6e2e656d7074792e6669656c64 = { "VAD" } response has no non-empty { $field }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e7661642e726573706f6e73652e6861732e6e6f2e6e6f6e2e6e656761746976652e66696e6974652e6669656c64 = { "VAD" } response has no non-negative finite { $field }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e7661642e726573706f6e73652e6861732e6e6f2e7370656563682e6172726179 = { "VAD" } response has no speech array

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e7661642e726573706f6e73652e6d7573742e62652e616e2e6f626a656374 = { "VAD" } response must be an object

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e7661642e726573706f6e73652e7761732e6e6f742e6a736f6e2e6572726f72 = { "VAD" } response was not { "JSON" }: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e7661642e7370656563682e656e7472792e656e64732e6265666f72652e69742e737461727473 = { "VAD" } speech entry ends before it starts

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e7661642e7370656563682e656e7472792e6861732e6e6f2e76616c69642e6669656c642e73616d706c652e696e646578 = { "VAD" } speech entry has no valid { $field } sample index

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e7661642e7370656563682e656e7472792e6d7573742e62652e616e2e6f626a656374 = { "VAD" } speech entry must be an object

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e776169742e6661696c6564 = could not wait for { "VAD" } helper: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7661642e7772697465 = could not write { "VAD" } input: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e766f6963652e77616974 = voice-only transcription is waiting: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e61646d697373696f6e2e646561646c696e652e6578636565646564 = bounded helper admission did not complete before its deadline

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e626c6f636b6564 = { $detail }; prior helper cleanup blocks admission: { $blockers }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e63616e63656c6c6564 = bounded helper was cancelled

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e636c65616e7570 = { $detail }; cleanup was incomplete at return: { $cleanup }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e636c65616e75702e626c6f636b6564 = { $detail }; cleanup was incomplete at return: { $cleanup }; prior helper cleanup blocks admission: { $blockers }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e636c65616e75702e636f6e74656e646564 = bounded helper cleanup admission is contended

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e636c65616e75702e70656e64696e67 = earlier bounded helper cleanup blocks admission

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e63757272656e742e6469726563746f72792e6e6f742e6469726563746f7279 = bounded helper current directory is not a directory

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e63757272656e742e6469726563746f72792e6f7574736964652e7061636b616765 = bounded helper current directory is outside its package root

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e63757272656e742e6469726563746f72792e756e617661696c61626c65 = bounded helper current directory could not be canonicalized

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e646561646c696e652e6578636565646564 = bounded helper did not complete before its deadline

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e65786563757461626c652e6e6f742e66696c65 = bounded helper executable is not a regular file

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e65786563757461626c652e6f7574736964652e7061636b616765 = bounded helper executable is outside its package root

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e65786563757461626c652e756e617661696c61626c65 = bounded helper executable could not be canonicalized

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e696e7075742e6c696d69742e6578636565646564 = bounded helper input exceeds its declared byte limit

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e696e7075742e77726974652e6661696c6564 = bounded helper input writer failed

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e696f2e776f726b65722e73746172742e6661696c6564 = bounded helper { $stream } { "I/O" } worker could not start

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e6a6f622e6e6f742e717569657363656e74 = bounded helper completion did not establish Job quiescence

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e6c61756e63682e6661696c6564 = bounded helper failed before an owned child identity was available

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e6c61756e63682e66696e616c697a6174696f6e2e6661696c6564 = native launch finalization failed: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e6d697373696e672e73797374656d2e726f6f74 = bounded helper environment must contain a nonempty { "SystemRoot" }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e6f75747075742e6c696d69742e6578636565646564 = bounded helper { $stream } output exceeded its declared byte limit

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e6f75747075742e726561642e6661696c6564 = bounded helper { $stream } output reader failed

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e7061636b6167652e726f6f742e756e617661696c61626c65 = bounded helper package root could not be canonicalized

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e706174682e656e7669726f6e6d656e742e72656675736564 = bounded helper environment may not contain { "PATH" }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e706174682e6e6f742e726570726573656e7461626c65 = bounded helper path cannot be represented by the managed Windows command boundary

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e70726f636573732e6f62736572766174696f6e2e6661696c6564 = bounded helper process observation failed

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e7a65726f2e6c696d6974 = bounded helper { $stream } byte limit must be nonzero

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e77696e646f77732e7a65726f2e74696d656f7574 = bounded helper timeout must be nonzero

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e626173652e74696d652e75732e6f662e6461792e69732e6f7574736964652e612e646179 = { "base_time_us_of_day" } is outside a day

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e627974652e636f756e742e646f65732e6e6f742e6d617463682e7368617065 = { "byte_count" } does not match shape

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e627974652e636f756e742e69732e746f6f2e6c61726765 = { "byte_count" } is too large

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e63616e63656c6c6564 = transcript publication was cancelled before admission

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e64657374696e6174696f6e = destination already exists: { $path }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e64747970652e6d7573742e62652e666c6f617433322e6c65 = { "dtype" } must be { "float32-le" }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e6475726174696f6e2e616e642e6e6f6973792e6d7573742e62652e70726f76696465642e746f676574686572 = duration and noisy must be provided together

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e6475726174696f6e732e732e636f756e742e646f65732e6e6f742e6d617463682e7368617065 = { "durations_s" } count does not match shape

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e6475726174696f6e732e732e6d7573742e62652e66696e6974652e6633322e76616c756573 = { "durations_s" } must be finite { "f32" } values

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e656e636f6465722e6d7573742e62652e6e6f6e656d707479 = encoder must be nonempty

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e6669656c642e6d7573742e62652e612e626f6f6c65616e = { $field } must be a boolean

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e6669656c642e6d7573742e62652e612e66696e6974652e6e756d626572 = { $field } must be a finite number

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e6669656c642e6d7573742e62652e612e737472696e67 = { $field } must be a string

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e6669656c642e6d7573742e62652e616e2e6172726179 = { $field } must be an array

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e6669656c642e6d7573742e62652e616e2e696e7465676572 = { $field } must be an integer

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e6669656c642e6d7573742e62652e616e2e756e7369676e65642e696e7465676572 = { $field } must be an unsigned integer

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e6865616465722e6e756d626572732e6d7573742e62652e66696e697465 = header numbers must be finite

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e69642e72616e6765 = id must be in { "1..=i32::MAX" }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e696e76616c69642e6633322e7061796c6f61642e6368756e6b = invalid { "f32" } payload chunk

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e6a736f6e6c2e706174682e616e642e6e707a2e706174682e6d7573742e62652e7369626c696e672e66696c6573 = { "jsonl_path" } and { "npz_path" } must be sibling files sharing a stem

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e6d697373696e672e72657175697265642e6669656c642e6b6579 = missing required field { $key }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e6e6f6973792e732e69732e72657175697265642e7768656e2e6e6f6973792e726d732e69732e70726573656e74 = { "noisy_s" } is required when { "noisy_rms" } is present

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e6e707a2e63616e63656c6c6564 = cancelled

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e6e707a2e636f756e74 = { "NPZ" } has an unexpected member count

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e6e707a2e6d656d62657273 = { "NPZ" } members do not match the transcript sidecar contract

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e6e707a2e6d697373696e67 = { "NPZ" } member { $name } is missing

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e6e707a2e706172656e74 = npz destination has no parent

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e6e707a2e726561646261636b = { "NPZ" } member { $name } did not round-trip

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e6f75747075742e706174682e6861732e6e6f2e706172656e74 = output path has no parent

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e6f75747075742e70617468732e6d7573742e62652e6e6f6e656d707479 = output paths must be nonempty

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7061796c6f61642e66696e697465 = embedding payload is non-finite at row { $row }, column { $column }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7061796c6f61642e666f726d61742e6d7573742e62652e7261772e6633326c652e726f772e6d616a6f722e7631 = { "payload_format" } must be { "raw-f32le-row-major-v1" }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7061796c6f61642e73697a652e646f65732e6e6f742e6d617463682e627974652e636f756e74 = payload size does not match { "byte_count" }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7072696f722e6a736f6e6c2e736e617073686f742e69732e6d697373696e67 = prior jsonl snapshot is missing

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7072696f722e6e707a2e736e617073686f742e69732e6d697373696e67 = prior npz snapshot is missing

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7072696f722e726573746f72652e72656675736564 = prior restore refused

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e707269766174652e73746167652e69732e6d697373696e67 = private stage is missing

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e707269766174652e7472616e7363726970742e636f70792e6469642e6e6f742e6d617463682e7468652e7374616765642e66696c65 = private transcript copy did not match the staged file

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e707269766174652e7472616e7363726970742e6a736f6e6c2e69732e6d697373696e67 = private transcript jsonl is missing

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e726f756e6465642e6865616465722e6e756d6265722e69732e6e6f742e6a736f6e2e726570726573656e7461626c65 = rounded header number is not { "JSON" }-representable

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e736368656d61 = unknown schema: { $schema }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7365676d656e742e6d6574612e6d7573742e62652e616e2e6f626a656374 = { "segment_meta" } must be an object

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e73686170652e627974652e636f756e742e6f766572666c6f7773 = shape byte count overflows

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e73686170652e6d7573742e62652e726f772e636f756e742e7769647468 = shape must be { "[row_count, 256]" }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e73686170652e726f772e636f756e742e69732e746f6f2e6c61726765 = shape row count is too large

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e737065616b65722e65766964656e63652e6d756c74692e6672616374696f6e2e69732e72657175697265642e776974682e737065616b65722e65766964656e6365 = { "speaker_evidence_multi_fraction" } is required with { "speaker_evidence" }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e737065616b65722e65766964656e63652e76657273696f6e2e69732e72657175697265642e776974682e737065616b65722e65766964656e6365 = { "speaker_evidence_version" } is required with { "speaker_evidence" }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e737065616b65722e6d7573742e62652e612e737472696e672e696e74656765722e6e756c6c2e6f722e616273656e74 = speaker must be a string, integer, null, or absent

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7370656563682e6c6f75642e77696e646f77732e69732e72657175697265642e7768656e2e6c6f75642e77696e646f77732e6973 = { "speech_loud_windows" } is required when { "loud_windows" } is positive

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e73746167652e706174682e616c72656164792e657869737473 = stage path already exists

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e73746167652e706174682e69732e612e73796d6c696e6b = stage path is a symlink

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e73746167652e706174682e69732e6e6f742e612e707269766174652e6469726563746f7279 = stage path is not a private directory

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7374616c652e6e707a2e756e6c696e6b2e72656675736564 = stale npz unlink refused

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e73746174656d656e74 = statement { $index }: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e73746174656d656e742e6475706c6963617465 = statement { $index } duplicates id { $id }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e73746174656d656e742e69642e69732e6e6f742e612e7472616e7363726970742e73746174656d656e742e6964 = statement ID is not a transcript statement ID

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e73746174656d656e742e69642e69732e6f7574736964652e693332 = statement ID is outside { "i32" }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e73746174656d656e742e69642e6d697373696e67 = statement { $index } is missing id

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e73746174656d656e742e6964732e636f756e742e646f65732e6e6f742e6d617463682e7368617065 = { "statement_ids" } count does not match shape

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e73746174656d656e742e6964732e6d7573742e62652e756e69717565 = statement IDs must be unique

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e73746174656d656e742e746578742e656e6465642e6265666f72652e6974732e7472696d6d65642e6c656e677468 = statement text ended before its trimmed length

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e73746174656d656e742e746578742e69732e6e6f742e76616c69642e75746638 = statement text is not valid { "utf-8" }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e746578742e6d7573742e62652e612e6e6f6e656d7074792e737472696e67 = text must be a nonempty string

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7475726e2e617564696f2e6475726174696f6e2e732e6d7573742e62652e6e6f6e2e6e65676174697665 = { "audio_duration_s" } must be non-negative

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7475726e2e626f756e646172792e737573706563742e6d7573742e62652e747275652e69662e70726573656e74 = { "turn_boundary_suspect" } must be true if present

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7475726e2e65787065637465642e76657273696f6e2e65787065637465642e676f742e76657273696f6e = expected version { $expected }, got { $version }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7475726e2e6669656c642e69732e6d697373696e67 = { $field } is missing

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7475726e2e6669656c642e6d7573742e62652e66696e697465 = { $field } must be finite

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7475726e2e696e76616c6964 = invalid { "speaker_turn_evidence" }: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7475726e2e72616e67652e696e6465782e656e642e732e656e642e732e657863656564732e617564696f = range { $index } { "end_s" } { $end_s } exceeds { "audio_duration_s" } { $audio_duration_s }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7475726e2e72616e67652e696e6465782e6861732e756e65787065637465642e6b657973 = range { $index } has unexpected keys

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7475726e2e72616e67652e696e6465782e69732e616e2e65786163742e6475706c6963617465 = range { $index } is an exact duplicate

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7475726e2e72616e67652e696e6465782e69732e696e7665727465642e73746172742e732e73746172742e73 = range { $index } is inverted: { "start_s" } { $start_s } > { "end_s" } { $end_s }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7475726e2e72616e67652e696e6465782e6d7573742e62652e616e2e6f626a656374 = range { $index } must be an object

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7475726e2e72616e67652e696e6465782e73746172742e732e6d7573742e62652e6e6f6e2e6e65676174697665 = range { $index } { "start_s" } must be non-negative

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7475726e2e72616e6765732e6d7573742e62652e616e2e6172726179 = ranges must be an array

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7475726e2e72616e6765732e6d7573742e62652e736f727465642e72616e67652e696e6465782e69732e6f7574 = ranges must be sorted: range { $index } is out of order

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7475726e2e737065616b65722e7475726e732e65766964656e63652e6861732e756e65787065637465642e6b657973 = speaker turns evidence has unexpected keys

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7475726e2e737065616b65722e7475726e732e65766964656e63652e6d7573742e62652e616e2e6f626a656374 = speaker turns evidence must be an object

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7475726e2e76657273696f6e2e6d7573742e62652e612e737472696e67 = version must be a string

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7a69702e63656e7472616c2e65787472612e61742e65787472612e61742e6572726f72 = central extra at { $extra_at }: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7a69702e63656e7472616c2e6865616465722e61742e63656e7472616c2e6865616465722e73746172742e6572726f72 = central header at { $central_header_start }: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7a69702e63656e7472616c2e6e616d652e61742e6e616d652e61742e6572726f72 = central name at { $name_at }: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7a69702e63656e7472616c2e7265636f72642e657874656e64732e6265796f6e642e61726368697665 = central record extends beyond archive

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7a69702e63656e7472616c2e7369676e61747572652e61742e63656e7472616c2e6865616465722e7374617274 = central signature at { $central_header_start }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7a69702e63703433372e7363616c6172 = { "cp437" } scalar

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7a69702e666c61676765642e6e616d652e69732e6e6f742e75746638 = flagged name is not { "utf-8" }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7a69702e6c6f63616c2e6865616465722e61742e6865616465722e73746172742e6572726f72 = local header at { $header_start }: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7a69702e6c6f63616c2e7369676e61747572652e61742e6865616465722e7374617274 = local signature at { $header_start }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7a69702e7261772e616e642e6578706f7365642e63656e7472616c2e706f70756c6174696f6e732e6469736167726565 = raw and exposed central populations disagree

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7a69702e7261772e63656e7472616c2e6d656d6265722e7761732e6c6f73742e66726f6d2e7468652e7a6970 = raw central member was lost from the ZIP index

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7a69702e72657065617465642e6578706f7365642e63656e7472616c2e636f6f7264696e617465 = repeated exposed central coordinate

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7a69702e756e69636f64652e706174682e65787472612e6372632e6d69736d61746368 = unicode path extra crc mismatch

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7a69702e756e69636f64652e706174682e65787472612e69732e6e6f742e75746638 = unicode path extra is not { "utf-8" }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7a69702e756e69636f64652e706174682e65787472612e746f6f2e736d616c6c = unicode path extra too small

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7a69702e756e70726f766564 = zip member { $index } name unproved: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7a69702e756e7265616461626c65 = zip archive unreadable: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e6572726f722e7772697465722e7a69702e7a69702e6d656d6265722e6e616d652e636f6c6c6973696f6e2e6e616d65 = zip member name collision: { $name }

m6a6f75726e616c2e636c692e7472616e7363726962652e666565642e6661696c757265 = speaker candidate pool feed failed for { $path }: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e666565642e7061727365 = failed to parse transcript: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e666565642e706f6f6c2e6c6f636b = candidate pool lock failed: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e666565642e706f6f6c2e72656164 = candidate pool read failed: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e666565642e706f6f6c2e7769647468 = candidate pool centroid width mismatch

m6a6f75726e616c2e636c692e7472616e7363726962652e666565642e706f6f6c2e7772697465 = candidate pool write failed: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e666565642e72656164 = failed to read transcript: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e666565642e756e7265616461626c65 = completed transcript or embeddings could not be read

m6a6f75726e616c2e636c692e7472616e7363726962652e66696c652e6661696c6564 = { $file }: transcription failed ({ $reason }); details: { $output }

m6a6f75726e616c2e636c692e7472616e7363726962652e68656c70 = usage: { "solstone journal transcribe [-h] [--all] [--redo]" }{ "\u000a" }{ "                          [--backend {parakeet,parakeet-cpp,confidential}]" }{ "\u000a" }{ "                          [-v] [-d]" }{ "\u000a" }{ "                          [audio_path]" }{ "\u000a" }{ "\u000a" }Transcribe audio files using pluggable { "STT" } and native speaker analysis{ "\u000a" }{ "\u000a" }positional arguments:{ "\u000a" }  { "audio_path" }            Path to audio file in journal segment directory, e.g.{ "\u000a" }                        { "HHMMSS_LEN/audio.flac" }{ "\u000a" }{ "\u000a" }options:{ "\u000a" }{ "  " }{ "-h, --help" }{ "            " }show this help message and exit{ "\u000a" }{ "  " }{ "--all" }{ "                 " }Batch-transcribe all unprocessed audio segments in the{ "\u000a" }                        journal{ "\u000a" }{ "  " }{ "--redo" }{ "                " }Reprocess file, overwriting existing outputs{ "\u000a" }{ "  --backend {parakeet,parakeet-cpp,confidential}" }{ "\u000a" }                        { "STT" } backend to use (overrides config and resource-{ "\u000a" }                        aware auto default){ "\u000a" }{ "  " }{ "-v, --verbose" }{ "         " }Enable verbose output{ "\u000a" }{ "  " }{ "-d, --debug" }{ "           " }Enable debug logging

m6a6f75726e616c2e636c692e7472616e7363726962652e696e7374616c6c6174696f6e2e6361706162696c6974792e6d69736d61746368 = generation capability does not match the current installation

m6a6f75726e616c2e636c692e7472616e7363726962652e696e7374616c6c6174696f6e2e636f646564 = { $code }: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e696e7374616c6c6174696f6e2e66642e706c6174666f726d = { "generation-fd" }: unsupported platform

m6a6f75726e616c2e636c692e7472616e7363726962652e696e7374616c6c6174696f6e2e6772616e742e6b696e64 = unexpected generation grant kind

m6a6f75726e616c2e636c692e7472616e7363726962652e696e7374616c6c6174696f6e2e6772616e742e7265717569726564 = authenticated generation grant is missing or duplicated

m6a6f75726e616c2e636c692e7472616e7363726962652e696e7374616c6c6174696f6e2e68656c7065722e706172656e74 = { "helper-path" }: current executable has no parent directory

m6a6f75726e616c2e636c692e7472616e7363726962652e696e7374616c6c6174696f6e2e69642e656e636f64696e67 = non-{ "Unicode" } generation ID

m6a6f75726e616c2e636c692e7472616e7363726962652e696e7374616c6c6174696f6e2e6c6f636b2e6b696e64 = generation lock is not an ordinary file

m6a6f75726e616c2e636c692e7472616e7363726962652e696e7374616c6c6174696f6e2e6d657461646174612e6d616c666f726d6564 = malformed generation metadata

m6a6f75726e616c2e636c692e7472616e7363726962652e696e7374616c6c6174696f6e2e6d657461646174612e7061727469616c = partial generation metadata

m6a6f75726e616c2e636c692e7472616e7363726962652e696e7374616c6c6174696f6e2e6d657461646174612e7265717569726564 = generation metadata is missing

m6a6f75726e616c2e636c692e7472616e7363726962652e696e7374616c6c6174696f6e2e6d657461646174612e756e7665726966696564 = generation metadata has no authenticated launch capability

m6a6f75726e616c2e636c692e7472616e7363726962652e696e7374616c6c6174696f6e2e6f62736f6c657465 = obsolete { "Windows" } generation descriptor marker

m6a6f75726e616c2e636c692e7472616e7363726962652e696e7374616c6c6174696f6e2e706c6174666f726d = { "platform-unsupported" }: { $platform }/{ $architecture }

m6a6f75726e616c2e636c692e7472616e7363726962652e696e7374616c6c6174696f6e2e70726f76656e616e6365 = generation launch provenance does not match this root

m6a6f75726e616c2e636c692e7472616e7363726962652e696e7374616c6c6174696f6e2e7374616d70 = { "proof-metadata" }: { $path }: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e696e7374616c6c6174696f6e2e746f6b656e2e656e636f64696e67 = non-{ "Unicode" } generation token

m6a6f75726e616c2e636c692e7472616e7363726962652e6f776e65722e72756e6e696e67 = your journal is running and transcribes recordings itself. bring the recording in with '{ "solstone import <file>" }' instead.

m6a6f75726e616c2e636c692e7472616e7363726962652e6f776e65722e73746f70706564 = your journal isn't running. start it with '{ "solstone journal up" }', then bring the recording in with '{ "solstone import <file>" }'; your journal transcribes it.

m6a6f75726e616c2e636c692e7472616e7363726962652e73656c656374696f6e2e636f6e666c696374 = { "--all" } and { "audio_path" } are mutually exclusive

m6a6f75726e616c2e636c692e7472616e7363726962652e73656c656374696f6e2e6d697373696e67 = provide { "audio_path" } or { "--all" }

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e616c6c2e77696e646f77732e6661696c6564 = sound tagging failed for all windows: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e656e67696e652e616273656e74 = ced engine is not installed

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e6661696c6564 = sound tagging failed: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e68656c7065722e65786563757461626c65 = ced helper binary is not executable: { $path }

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e68656c7065722e65786974 = ced helper exited { $code }: { $stderr }

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e68656c7065722e696f = ced helper I/O failed: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e68656c7065722e6c61726765 = ced response exceeded 64 MiB

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e68656c7065722e6d697373696e67 = ced helper binary is missing: { $path }

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e68656c7065722e706172656e74 = current executable has no parent directory

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e68656c7065722e726573706f6e7365 = ced helper response was not valid JSON: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e68656c7065722e737061776e = ced helper could not start: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e68656c7065722e74696d656f7574 = ced helper did not exit within { $timeout }

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e68656c7065722e756e7265736f6c766564 = ced helper unresolved: { $detail }

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e68656c7065722e77696e646f7773 = CED requires the Windows host caller

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e7061636b6167652e706c6174666f726d = unsupported CED platform { $os }/{ $arch }

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e7061636b6167652e77696e646f77732e72756e74696d65 = Windows CED package verification requires a Windows runtime

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e70726570617265 = sound tagging could not prepare audio: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e70726f62652e68656c7065722e65786974 = ced probe helper exited { $code }: { $stderr }

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e70726f62652e68656c7065722e756e6578706563746564 = ced probe helper returned an unexpected response: { $response }

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e70726f62652e756e6578706563746564 = ced probe returned an unexpected response: { $response }

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e736368656d61 = sound tagging returned an unexpected response schema

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e756e737570706f72746564 = sound tagger disabled: ced assets unsupported on { $os }/{ $arch }

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e77696e646f772e636f756e74 = sound tagging returned an unexpected window count

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e77696e646f772e6f6b = sound tagging returned a window missing an { "ok" } field

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e77696e646f77732e6c61756e6368 = CED helper launch failed: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e77696e646f77732e6c696272617279 = CED request library does not match the signed package

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e77696e646f77732e7061636b6167652e62696e = running executable is not in the package bin directory

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e77696e646f77732e7061636b6167652e6d656d626572 = signed CED app payload refuses { $member }: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e77696e646f77732e7061636b6167652e706172656e74 = running executable has no containing directory

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e77696e646f77732e7061636b6167652e726f6f74 = package bin directory has no package root

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e77696e646f77732e70726f6265 = CED helper returned an invalid probe response

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e77696e646f77732e726f6f74 = { "SystemRoot" } is not set

m6a6f75726e616c2e636c692e7472616e7363726962652e736f756e642e77696e646f77732e7369676e6564 = Windows CED requires the signed package helper and its declared command

m6a6f75726e616c2e636c692e7472616e7363726962652e73746167652e636f6e666964656e7469616c2e636170 = audio is { $seconds }s and would send { $bytes } bytes; confidential STT accepts at most { $limit } bytes

m6a6f75726e616c2e636c692e7472616e7363726962652e73746167652e636f6e666964656e7469616c2e766572696669636174696f6e2e636c656172 = confidential transcription verification was not cleared for { $path }: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e73746167652e636f6e666964656e7469616c2e766572696669636174696f6e2e7265636f7264 = confidential transcription verification was not recorded for { $path }: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e73746167652e656d6265646465642e6d697373696e672e6472616674 = recognize walk has no draft spool

m6a6f75726e616c2e636c692e7472616e7363726962652e73746167652e656d6265646465642e72616e67652e6d697373696e67 = merged turn range is missing

m6a6f75726e616c2e636c692e7472616e7363726962652e73746167652e656d6265646465642e73637261746368 = could not create embedded scratch: { $error }

m6a6f75726e616c2e636c692e7472616e7363726962652e73746167652e6f75747075742e706172656e74 = transcript path has no parent

m6a6f75726e616c2e636c692e7472616e7363726962652e73746167652e6f776e65722e64656c657465642e756e7265636f72646564 = voice-only original deleted but deletion event was not recorded

m6a6f75726e616c2e636c692e7472616e7363726962652e73746167652e6f776e65722e68656c7065722e6f75746461746564 = the speaker-analysis helper predates overlap ranges or speaker turns

m6a6f75726e616c2e636c692e7472616e7363726962652e73746167652e6f776e65722e6e6f742e64656c65746564 = voice-only original not deleted now: { $outcome }

m6a6f75726e616c2e636c692e7472616e7363726962652e73746167652e6f776e65722e736176652e6f722e64697361626c65 = save a voice in Speakers, or turn off { "transcribe.owner_voice_only" }

m6a6f75726e616c2e636c692e7472616e7363726962652e73746167652e6f776e65722e736176652e746f2e66696e697368 = save a voice in Speakers to finish this filtered recording

m6a6f75726e616c2e636c692e7472616e7363726962652e73756d6d617279 = { $processed } processed, { $skipped } skipped (already transcribed)

m6a6f75726e616c2e636c692e7472616e7363726962652e73756d6d6172792e6465666572726564 = { $processed } processed, { $skipped } skipped (already transcribed), { $deferred } deferred (provider not ready, will retry)

m6a6f75726e616c2e636c692e7472616e7363726962652e73756d6d6172792e64656665727265642e6661696c6564 = { $processed } processed, { $skipped } skipped (already transcribed), { $deferred } deferred (provider not ready, will retry), { $failed } failed

m6a6f75726e616c2e636c692e7472616e7363726962652e73756d6d6172792e6661696c6564 = { $processed } processed, { $skipped } skipped (already transcribed), { $failed } failed

m6a6f75726e616c2e636c692e7472616e7363726962652e75736167652e6572726f72 = usage: { "solstone journal transcribe [-h] [--all] [--redo]" }{ "\u000a" }{ "                          [--backend {parakeet,parakeet-cpp,confidential}]" }{ "\u000a" }{ "                          [-v] [-d]" }{ "\u000a" }{ "                          [audio_path]" }{ "\u000a" }{ "solstone journal transcribe" }: error: { $message }

