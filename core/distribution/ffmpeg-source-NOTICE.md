# FFmpeg source and licence notice

This journal build statically links FFmpeg source at commit
`76c070aa057b590b75343e848f8b98b03c496572`. The pinned source archive is
`FFmpeg-76c070aa057b590b75343e848f8b98b03c496572.tar.gz`
(SHA-256 `65f6c555fca9cb5276e0d6775272b98678959786769d7cd1b151709d14aa685f`).
It is available from
https://github.com/FFmpeg/FFmpeg/archive/76c070aa057b590b75343e848f8b98b03c496572.tar.gz.

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
