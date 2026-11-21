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

