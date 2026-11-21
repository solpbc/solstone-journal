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

m6a6f75726e616c2e636c692e636c69656e742e6572726f722e6d616c666f726d6564 = the journal response couldn't be read.

m6a6f75726e616c2e636c692e636c69656e742e6572726f722e74696d656f7574 = The journal didn't answer in time.

m6a6f75726e616c2e636c692e636c69656e742e6572726f722e756e726561636861626c65 = your journal couldn't be reached over { "HTTP" }.

m6a6f75726e616c2e636c692e636c69656e742e6572726f722e756e7265616461626c65 = The journal returned an unreadable error.

m6a6f75726e616c2e636c692e636c69656e742e706f72742e696e76616c6964 = { "SOLSTONE_CONVEY_PORT" } must be an integer from 1 to 65535.

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

