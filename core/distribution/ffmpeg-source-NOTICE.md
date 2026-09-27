# FFmpeg source and licence notice

This journal build statically links FFmpeg source at commit
`03d9533176e98bb9fbf569c1f34968e73e948dd9`. The pinned source archive is
`FFmpeg-03d9533176e98bb9fbf569c1f34968e73e948dd9.tar.gz`
(SHA-256 `ba7070db2f8a0590e3bbad428c8ffbec33f80b0a430bcad79c2cfb756e84ff8b`).
It is available from
https://github.com/FFmpeg/FFmpeg/archive/03d9533176e98bb9fbf569c1f34968e73e948dd9.tar.gz.

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
