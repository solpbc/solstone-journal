# FFmpeg source and licence notice

This journal build statically links FFmpeg source at commit
`946fcce07b6dcd0331c8cc609192aeff5e1924f8`. The pinned source archive is
`FFmpeg-946fcce07b6dcd0331c8cc609192aeff5e1924f8.tar.gz`
(SHA-256 `0aa2b1de2a5698b20a23e93d539a9a8e82ca0117496c5bdf05d198805f42bb3b`).
It is available from
https://github.com/FFmpeg/FFmpeg/archive/946fcce07b6dcd0331c8cc609192aeff5e1924f8.tar.gz.

The FFmpeg libraries in this build use LGPL version 2.1 or later. Their licence
text is in `COPYING.LGPLv2.1`; FFmpeg's own licensing and copyright information
is in `LICENSE.md` and the pinned source files. The other COPYING files from
the same source archive accompany them for reference.

The `<package-name>.release` file beside this journal package records the exact
journal source commit as `commit=`. Get that revision of the complete journal program source,
including its FFmpeg build configuration, from
https://github.com/solpbc/solstone-journal. The pinned FFmpeg archive above
provides the corresponding FFmpeg source. Together they provide the source
needed to rebuild and relink the journal with a modified FFmpeg library.
