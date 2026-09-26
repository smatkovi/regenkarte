#!/bin/sh
set -e
SRC=/tmp/kartendienst-src
. "$SRC/tools/cross.env"
if [ ! -x "$CARGO_HOME/bin/cargo" ] || [ ! -d "$MUSL" ]; then
    sh "$SRC/tools/toolchain.sh"
    . "$SRC/tools/cross.env"
fi
cd "$SRC"
cargo build --release --target "$ZIEL"
mkdir -p "$SRC/build"
cp "target/$ZIEL/release/kartendienst" "$SRC/build/"
