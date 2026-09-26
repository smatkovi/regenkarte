#!/bin/sh
# Richtet Rust und die musl-Cross-Toolchain ein -- auf dem Build-Rechner,
# unter /tmp.
#
# Warum /tmp und nicht das Heimverzeichnis: dessen Partition ist zu 99%
# voll (5 GB frei), /tmp ist ein tmpfs mit 31 GB. Der Preis ist, dass ein
# Neustart alles loescht; dieses Skript baut es in wenigen Minuten wieder
# auf.
set -e
. "$(dirname "$0")/cross.env"

if [ ! -x "$CARGO_HOME/bin/cargo" ]; then
    echo "== rustup"
    mkdir -p "$(dirname "$CARGO_HOME")"
    curl -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh
    sh /tmp/rustup-init.sh -y --no-modify-path --profile minimal \
        --default-toolchain stable
    "$CARGO_HOME/bin/rustup" target add "$ZIEL"
fi

if [ ! -d "$MUSL" ]; then
    echo "== musl-Cross-Toolchain"
    # Rust bringt libc.a und die crt-Objekte fuer musl mit, aber keine
    # Header -- und SQLite braucht stdio.h und pthread.h.
    cd "$(dirname "$MUSL")"
    curl -sSLO https://musl.cc/arm-linux-musleabi-cross.tgz
    tar xzf arm-linux-musleabi-cross.tgz
    rm -f arm-linux-musleabi-cross.tgz
fi
echo "== Toolchain bereit"
