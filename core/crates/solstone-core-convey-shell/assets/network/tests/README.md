Run automated tests with `make check-convey-pages`. Excluded pages are listed in `tools/convey-page-runner/exclude.txt`.

Open `network-delivery.html` in a browser; each assertion reports pass/fail inline
for `network.js`'s delivery-vs-connection classification: grouping, chip labels and
hues, the secondary connection line, check-in age, and the paired-day key.

These pages are not embedded in the binary — the crate's `build.rs` enumerates the
network assets it serves by name and never walks this directory.
