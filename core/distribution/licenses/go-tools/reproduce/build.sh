#!/bin/bash
# build.sh: regenerate out/ from bi/, work/*.dl.json, work/*.pk (see README-reproduce.md)
set -e
R=${GOLIC_ROOT:-/var/tmp/golic-r1}
for spec in restic:1.26.4:github.com/restic/restic rclone:1.26.5:github.com/rclone/rclone; do
  IFS=: read -r tool gover main <<<"$spec"
  python3 -I $R/scripts/collect.py $tool $gover $main
  (cd $R/out/$tool && find . -type f ! -name attribution.json -printf '%P\n' | sort > $R/work/$tool.files)
  (cd $R/out/$tool && $R/bin/classify $(cat $R/work/$tool.files) > $R/work/$tool.class.jsonl)
  python3 -I $R/scripts/assemble.py $tool $gover
done
