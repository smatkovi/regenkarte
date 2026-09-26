#!/bin/sh
# Baut den Kartendienst fuer das N9/N950 auf dem Build-Rechner.
set -e
cd "$(dirname "$0")/.."
FERN=/tmp/kartendienst-src
HOST=$(sh "$HOME/ps/nfsshift-sfos/tools/buildhost.sh")
echo "== Build-Rechner: $HOST"
rsync -a --delete --exclude build --exclude target --exclude .git ./ "$HOST:$FERN/"
ssh "$HOST" 'sh /tmp/kartendienst-src/tools/remote-build.sh'
mkdir -p build
scp -q "$HOST:$FERN/build/kartendienst" build/
echo "== kartendienst fertig ($(stat -c %s build/kartendienst) B)"
