#!/bin/sh
# Started in the background by the install scripts during an in-app update. Waits for the old
# app (pid $1) to exit, opens the new build with the remaining arguments, then checks it is
# still running a few seconds later and tries once more if it is not. Output goes to the
# update log the app opened, so a failed restart leaves a trace there.
pid=$1
shift
while kill -0 "$pid" 2>/dev/null; do sleep 0.2; done
for attempt in 1 2; do
    "$@"
    sleep 5
    if pgrep -x scytify >/dev/null; then
        echo "Relaunched."
        exit 0
    fi
    echo "The new build was not running 5 s after launch (attempt $attempt)."
done
exit 1
