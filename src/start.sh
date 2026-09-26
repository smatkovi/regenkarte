#!/bin/sh
# Regenkarte-Starter: Kachel-Dienst bei Bedarf hochziehen, dann die
# Oberflaeche. Der Dienst ist seit 2.0 ein eigenes Programm und braucht
# kein Python mehr -- weder das des Geraets noch das aus /opt/wunderw.
DIR=/opt/regenkarte
if ! "$DIR/kartendienst" probe 2>/dev/null; then
    "$DIR/kartendienst" serve >/tmp/regenkarte-serve.log 2>&1 &
fi
cd $DIR
exec /usr/bin/python $DIR/launcher.py $DIR/main.qml
