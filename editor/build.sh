#!/bin/sh
# Rebuild static/editor.js.gz from entry.js (needs node + npm).
set -eu
cd "$(dirname "$0")"
npm install --no-audit --no-fund
./node_modules/.bin/esbuild entry.js --bundle --minify --format=iife --global-name=HoloEditor \
  --target=es2020 --legal-comments=eof --outfile=editor.js
gzip -9n -c editor.js > ../static/editor.js.gz
rm editor.js
echo "wrote ../static/editor.js.gz"
